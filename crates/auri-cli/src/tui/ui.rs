//! What the terminal shows: header, band table, response plot, verdict, keys.

use auri_gate::{GateResult, Profile};
use auri_policy::{FLASH_NOTICE_TEXT, ItemId, LOCKED_TEXT, LOUDNESS_WARNING_TEXT, LoudnessReason};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use crate::presets;
use crate::tui::app::{App, Col, Focus};
use crate::tui::plot;

/// The number of reason lines the verdict block can show before it truncates.
const MAX_REASON_LINES: usize = 4;

/// Renders one frame.
pub fn render(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(6),
            Constraint::Length(12),
            Constraint::Length(3),
        ])
        .split(area);

    header(frame, app, rows[0]);
    body(frame, app, rows[1]);
    verdict(frame, app, rows[2]);
    footer(frame, app, rows[3]);
}

fn header(frame: &mut Frame, app: &App, area: Rect) {
    let layer = if app.experimental {
        "engineering"
    } else {
        "ajustar"
    };
    let armed = if app.armed { "  ARMED" } else { "" };
    let mut spans = vec![
        Span::styled("profile ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            app.profile.as_str(),
            Style::default()
                .fg(yellow(app.profile))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("   layer ", Style::default().fg(Color::DarkGray)),
        Span::styled(layer, Style::default().fg(Color::Cyan)),
    ];
    // A read-only marker (like eqtui's): the curve is an official preset,
    // verbatim. It is cosmetic — writing is gated exactly as for any curve.
    if let Some(preset) = app
        .preset_index
        .and_then(|index| presets::PRESETS.get(index))
    {
        spans.push(Span::styled(
            "   preset ",
            Style::default().fg(Color::DarkGray),
        ));
        spans.push(Span::styled(
            format!("{} [RO]", preset.label),
            Style::default().fg(Color::Cyan),
        ));
    }
    spans.extend(vec![
        Span::styled("   preamp ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!("{:+.1} dB", app.preamp_db),
            Style::default().fg(Color::White),
        ),
        Span::styled("   bands ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!("{}/10 active", active_count(app)),
            Style::default().fg(Color::White),
        ),
        Span::styled(
            armed,
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ),
    ]);
    let line = Line::from(spans);

    let block = Block::bordered().title(" kuba auri — CB5100 DSP ");
    frame.render_widget(Paragraph::new(line).block(block), area);
}

fn body(frame: &mut Frame, app: &App, area: Rect) {
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(62), Constraint::Percentage(38)])
        .split(area);
    bands(frame, app, columns[0]);
    response(frame, app, columns[1]);
}

fn bands(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::bordered().title(" bands  hjkl move  +- step  c kind  x off  { } preset ");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let mut lines = Vec::with_capacity(app.bands.len());
    for (index, band) in app.bands.iter().enumerate() {
        let selected_row = index == app.row;
        let mut spans = Vec::with_capacity(5);

        spans.push(cell(
            format!("{:>2} ", index + 1),
            selected_row,
            false,
            Style::default().fg(if selected_row {
                Color::Yellow
            } else {
                Color::DarkGray
            }),
        ));

        spans.push(cell(
            format!("{:<4} ", band.kind().as_str()),
            selected_row,
            selected_row && app.col == Col::Kind,
            Style::default().fg(Color::Cyan),
        ));

        spans.push(cell(
            match band.freq_hz() {
                Some(hz) if hz >= 1_000.0 => format!("{:>8.2}k ", hz / 1_000.0),
                Some(hz) => format!("{hz:>7.1}Hz "),
                None => "        — ".to_string(),
            },
            selected_row,
            selected_row && app.col == Col::Freq,
            Style::default().fg(Color::White),
        ));

        spans.push(cell(
            if band.kind().has_gain() {
                format!("{:+>8.1}dB ", band.gain_db())
            } else {
                "        — ".to_string()
            },
            selected_row,
            selected_row && app.col == Col::Gain,
            Style::default().fg(if band.gain_db() > 0.0 {
                Color::LightRed
            } else if band.gain_db() < 0.0 {
                Color::LightBlue
            } else {
                Color::White
            }),
        ));

        spans.push(cell(
            match band.q_factor() {
                Some(q) => format!("{q:>5.2} "),
                None => "    — ".to_string(),
            },
            selected_row,
            selected_row && app.col == Col::Q,
            Style::default().fg(Color::White),
        ));

        lines.push(Line::from(spans));
        if lines.len() as u16 >= inner.height {
            break;
        }
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

/// One table cell: highlighted when it holds the cursor, dimmed otherwise.
fn cell(text: String, row: bool, cursor: bool, style: Style) -> Span<'static> {
    if cursor {
        Span::styled(
            text,
            Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
    } else if row {
        Span::styled(text, style.add_modifier(Modifier::BOLD))
    } else {
        Span::styled(text, style)
    }
}

fn response(frame: &mut Frame, app: &App, area: Rect) {
    let width = area.width.saturating_sub(2) as usize;
    let height = area.height.saturating_sub(2) as usize;

    let samples = plot::sample(&app.bands, app.preamp_db, (width * 2).max(16));
    let values: Vec<f64> = samples.iter().map(|&(_, db)| db).collect();
    let (lo, hi) = plot::window(&values);

    let block = Block::bordered().title(format!(
        " response  {lo:+.1} … {hi:+.1} dB  20 Hz – 20 kHz "
    ));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if width == 0 || height == 0 {
        return;
    }

    let art = plot::braille(&values, width, height, lo, hi);
    let mut rows: Vec<Line> = art
        .rows()
        .into_iter()
        .map(|row| Line::from(Span::styled(row, Style::default().fg(Color::Green))))
        .collect();

    // The 0 dB reference, drawn under whatever the curve put there.
    if let Some(zero) = plot::zero_row(lo, hi, height)
        && let Some(line) = rows.get_mut(zero)
    {
        let mut text = String::with_capacity(width);
        for column in 0..width {
            text.push(if art.at(column, zero) == ' ' {
                '─'
            } else {
                '·'
            });
        }
        *line = Line::from(Span::styled(text, Style::default().fg(Color::DarkGray)));
    }

    frame.render_widget(Paragraph::new(rows), inner);
}

fn verdict(frame: &mut Frame, app: &App, area: Rect) {
    let result = &app.verdict;
    let mut lines = Vec::with_capacity(MAX_REASON_LINES + 8);

    lines.push(status_line(result, app.profile));

    for reason in result.reasons.iter().take(MAX_REASON_LINES) {
        lines.push(Line::from(vec![
            Span::styled("  ✗ ", Style::default().fg(Color::Red)),
            Span::styled(
                crate::describe(reason),
                Style::default().fg(Color::LightRed),
            ),
        ]));
    }
    if result.reasons.len() > MAX_REASON_LINES {
        lines.push(Line::from(Span::styled(
            format!("  … and {} more", result.reasons.len() - MAX_REASON_LINES),
            Style::default().fg(Color::DarkGray),
        )));
    }
    if let Some(waived) = &result.waived_reasons {
        let shown = waived.len().min(2);
        for reason in waived.iter().take(shown) {
            lines.push(Line::from(vec![
                Span::styled("  ~ ", Style::default().fg(Color::Yellow)),
                Span::styled(crate::describe(reason), Style::default().fg(Color::Yellow)),
            ]));
        }
        if waived.len() > shown {
            lines.push(Line::from(Span::styled(
                format!("  … and {} waived", waived.len() - shown),
                Style::default().fg(Color::DarkGray),
            )));
        }
    }

    lines.push(level_line(app));
    if app.checklist.loudness.warn {
        lines.push(Line::from(Span::styled(
            LOUDNESS_WARNING_TEXT,
            Style::default().fg(Color::LightYellow),
        )));
    }
    lines.push(confirmations_line(app));
    if app.checklist.gate_ok && !app.checklist.can_write {
        lines.push(Line::from(Span::styled(
            format!("  {LOCKED_TEXT}"),
            Style::default().fg(Color::DarkGray),
        )));
    }
    lines.push(counters_line(app));
    if app.checklist.daily_notice {
        lines.push(Line::from(Span::styled(
            FLASH_NOTICE_TEXT,
            Style::default().fg(Color::Yellow),
        )));
    }

    let mut block = Block::bordered().title(" gate ");
    if app.focus == Focus::Checklist {
        // Cyan, never yellow: the yellow background belongs to the table cursor.
        block = block.border_style(Style::default().fg(Color::Cyan));
    }
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn status_line(result: &GateResult, profile: Profile) -> Line<'static> {
    let (label, colour) = if result.ok {
        ("PASS", Color::Green)
    } else {
        ("REFUSED", Color::Red)
    };
    let crc = result
        .metrics
        .payload_crc
        .clone()
        .unwrap_or_else(|| "—".to_string());
    let blocking = result.reasons.len();

    Line::from(vec![
        Span::styled(
            format!("gate: {label}"),
            Style::default().fg(colour).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  ({}) ", profile.as_str()),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled("CRC ", Style::default().fg(Color::DarkGray)),
        Span::styled(crc, Style::default().fg(Color::Magenta)),
        Span::styled(
            format!("  {blocking} blocking"),
            Style::default().fg(if blocking == 0 {
                Color::DarkGray
            } else {
                Color::Red
            }),
        ),
    ])
}

/// The predicted level of this curve, what the last write left behind and
/// the jump between them. Computed data, not audio.
fn level_line(app: &App) -> Line<'static> {
    let checklist = &app.checklist;
    let level = checklist
        .new_level_db
        .map_or_else(|| "—".to_string(), |value| format!("{value:+.2} dB"));
    let last = checklist
        .last_written_level_db
        .map_or_else(|| "—".to_string(), |value| format!("{value:+.2} dB"));
    let jump = checklist
        .loudness
        .delta_db
        .map_or_else(|| "—".to_string(), |value| format!("{value:+.2} dB"));
    let jump_colour = if checklist.loudness.reasons.contains(&LoudnessReason::Jump) {
        Color::LightYellow
    } else {
        Color::DarkGray
    };

    Line::from(vec![
        Span::styled(
            "  level 300 Hz–3 kHz ",
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(
            level,
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("   last written ", Style::default().fg(Color::DarkGray)),
        Span::styled(last, Style::default().fg(Color::DarkGray)),
        Span::styled("   jump ", Style::default().fg(Color::DarkGray)),
        Span::styled(jump, Style::default().fg(jump_colour)),
    ])
}

/// The tickable confirmations on one line: `[x]` when satisfied, reversed on
/// the row the checklist cursor holds.
fn confirmations_line(app: &App) -> Line<'static> {
    let visible = app.visible_confirmations();
    let mut spans = vec![Span::styled("  ", Style::default())];
    if visible.is_empty() {
        spans.push(Span::styled(
            "nothing to confirm",
            Style::default().fg(Color::DarkGray),
        ));
    }
    for (index, id) in visible.iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled("   ", Style::default()));
        }
        let checked = app.checklist.item(*id).is_some_and(|item| item.satisfied);
        let mut style = Style::default().fg(if checked {
            Color::Green
        } else {
            Color::DarkGray
        });
        if app.focus == Focus::Checklist && index == app.confirm_row {
            style = style.add_modifier(Modifier::REVERSED);
        }
        spans.push(Span::styled(
            format!(
                "{} {}",
                if checked { "[x]" } else { "[ ]" },
                confirm_label(*id)
            ),
            style,
        ));
    }
    Line::from(spans)
}

const fn confirm_label(id: ItemId) -> &'static str {
    match id {
        ItemId::Volume => "volume lowered",
        ItemId::Connection => "keep connected",
        ItemId::Experimental => "experimental",
        ItemId::YoloSession => "yolo session",
        ItemId::YoloWrite => "yolo write",
        ItemId::Gate | ItemId::Spacing => "",
    }
}

/// Flash counters, the 2 s spacing and where the next write would go.
fn counters_line(app: &App) -> Line<'static> {
    let counters = app.checklist.counters;
    let spacing = app.checklist.spacing;
    let wait = if spacing.ok {
        "ready".to_string()
    } else {
        format!("{:.1}s", spacing.wait_ms as f64 / 1000.0)
    };

    Line::from(vec![
        Span::styled(
            format!(
                "  {} this session · {} today · {} in total",
                counters.session, counters.day, counters.total
            ),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled("   spacing ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            wait,
            Style::default().fg(if spacing.ok {
                Color::Green
            } else {
                Color::Yellow
            }),
        ),
        Span::styled("   to ", Style::default().fg(Color::DarkGray)),
        Span::styled(app.target.clone(), Style::default().fg(Color::Cyan)),
    ])
}

fn footer(frame: &mut Frame, app: &App, area: Rect) {
    let line = Line::from(vec![
        Span::styled(&app.status, Style::default().fg(Color::White)),
        Span::styled("   ", Style::default()),
        Span::styled(
            "hjkl/gG move · +/- step · c kind · x off · t yolo · e layer · n neutro · r reset · w write · ? help · q quit",
            Style::default().fg(Color::DarkGray),
        ),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn yellow(profile: Profile) -> Color {
    match profile {
        Profile::Standard => Color::Green,
        Profile::Yolo => Color::Yellow,
    }
}

fn active_count(app: &App) -> usize {
    app.bands
        .iter()
        .filter(|band| band.kind() != auri_codec::BandKind::Off)
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::Key;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn terminal() -> Terminal<TestBackend> {
        Terminal::new(TestBackend::new(80, 24)).expect("test backend")
    }

    /// Renders a frame and returns everything the test could see in it.
    fn screen(app: &App) -> String {
        let mut terminal = terminal();
        terminal.draw(|frame| render(frame, app)).expect("draw");
        let buffer = terminal.backend().buffer().clone();
        let mut out = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                out.push_str(buffer[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    /// Neutral, spaced-in, confirmations ticked: `w` arms.
    fn ready() -> App {
        let mut app = App::neutral(false);
        app.tick(2_000);
        app.confirmations.volume_lowered = true;
        app.confirmations.keep_connected = true;
        app.refresh_checklist();
        app
    }

    #[test]
    fn a_frame_renders_the_header_table_plot_and_keys() {
        let text = screen(&App::neutral(false));
        assert!(text.contains("kuba auri"), "header missing");
        assert!(text.contains("gate: PASS"), "verdict missing: {text}");
        assert!(text.contains("response"), "plot missing");
        assert!(text.contains("hjkl"), "key hints missing");
        assert!(text.contains("CRC"), "crc missing");
    }

    #[test]
    fn a_refusal_is_visible_in_red_words() {
        let mut app = App::neutral(false);
        app.preamp_db = -0.2;
        app.recompute();
        assert!(!app.verdict.ok);
        let text = screen(&app);
        assert!(text.contains("gate: REFUSED"), "{text}");
        assert!(
            text.contains("g:preamp"),
            "the reason must be named: {text}"
        );
    }

    #[test]
    fn arming_shows_up_and_cancelling_clears_it() {
        let mut app = ready();
        app.handle_key(Key::Char('w'));
        let armed = screen(&app);
        assert!(armed.contains("ARMED"), "{armed}");

        app.handle_key(Key::Esc);
        assert!(!screen(&app).contains("ARMED"));
    }

    #[test]
    fn a_preset_in_the_header_shows_the_read_only_marker() {
        let mut app = ready();
        assert!(!screen(&app).contains("[RO]"), "file curve: no marker");
        app.handle_key(Key::Char('}'));
        let text = screen(&app);
        assert!(text.contains("[RO]"), "preset must carry [RO]: {text}");
        assert!(
            text.contains(crate::presets::PRESETS[0].label),
            "the preset label must be named: {text}"
        );
    }

    #[test]
    fn the_checklist_shows_levels_confirmations_and_counters() {
        let text = screen(&App::neutral(false));
        assert!(text.contains("level"), "level line: {text}");
        assert!(text.contains("volume lowered"), "confirmations: {text}");
        assert!(text.contains("this session"), "counters: {text}");
        assert!(text.contains("spacing"), "spacing: {text}");
        assert!(text.contains("Complete every item"), "lock message: {text}");

        let mut app = ready();
        app.handle_key(Key::Tab);
        let focused = screen(&app);
        assert!(focused.contains("[x]"), "ticked items: {focused}");
        assert!(!focused.contains("Complete every item"), "{focused}");
    }

    /// Every character cell that carries the cursor background.
    ///
    /// Only the cursor paints a yellow *background* — the plot and the YOLO
    /// header only tint their foreground — so this cannot pick up anything
    /// else. Note that a table cell is several characters wide, which is why
    /// this returns a list rather than a count of "cells".
    fn yellow_spots(app: &App) -> Vec<(u16, u16)> {
        let mut terminal = terminal();
        terminal.draw(|frame| render(frame, app)).expect("draw");
        let buffer = terminal.backend().buffer();
        (0..buffer.area.height)
            .flat_map(|y| (0..buffer.area.width).map(move |x| (x, y)))
            .filter(|&(x, y)| buffer[(x, y)].bg == Color::Yellow)
            .collect()
    }

    /// The first character of the cursor, or `(u16::MAX, u16::MAX)`.
    fn cursor_cell(app: &App) -> (u16, u16) {
        yellow_spots(app)
            .first()
            .copied()
            .unwrap_or((u16::MAX, u16::MAX))
    }

    #[test]
    fn the_cursor_cell_moves_with_the_column_and_the_row() {
        let mut app = App::neutral(false);
        let kind = cursor_cell(&app);
        assert_ne!(kind, (u16::MAX, u16::MAX), "a cursor cell must exist");

        app.handle_key(Key::Char('l'));
        let freq = cursor_cell(&app);
        assert_ne!(freq, kind, "l must move the highlight to the right");

        app.handle_key(Key::Char('j'));
        let below = cursor_cell(&app);
        assert_ne!(below, freq, "j must move the highlight down");
        assert_eq!(below.0, freq.0, "j keeps the same column");
        assert!(below.1 > freq.1, "j increases the row");

        app.handle_key(Key::Char('h'));
        assert_eq!(cursor_cell(&app).0, kind.0, "h walks back to kind");
    }

    #[test]
    fn the_cursor_is_one_contiguous_cell_not_a_whole_column() {
        let mut app = App::neutral(false);
        for _ in 0..4 {
            app.handle_key(Key::Char('l'));
            let spots = yellow_spots(&app);
            assert!(!spots.is_empty(), "no cursor at {}", app.col.label());

            // A table cell is several characters wide, so "one cell" means one
            // row and one unbroken run of columns — never a whole column.
            let mut rows: Vec<u16> = spots.iter().map(|&(_, y)| y).collect();
            rows.sort_unstable();
            rows.dedup();
            assert_eq!(rows.len(), 1, "the cursor must sit on one row");

            let mut columns: Vec<u16> = spots.iter().map(|&(x, _)| x).collect();
            columns.sort_unstable();
            columns.dedup();
            assert!(
                columns.windows(2).all(|w| w[1] == w[0] + 1),
                "the cursor must be one unbroken run, got {columns:?}"
            );
        }

        let before = yellow_spots(&app);
        app.handle_key(Key::Char('j'));
        let after = yellow_spots(&app);
        assert!(
            after[0].1 > before[0].1,
            "j must move the cell down: {before:?} → {after:?}"
        );
    }

    #[test]
    fn an_off_band_shows_dashes_not_numbers() {
        let text = screen(&App::neutral(false));
        assert!(text.contains("OFF"), "kind column: {text}");
        assert!(text.contains('—'), "empty columns use an em dash");
    }

    #[test]
    fn rendering_never_panics_on_a_tiny_terminal() {
        let mut terminal = Terminal::new(TestBackend::new(10, 6)).expect("test backend");
        let app = App::neutral(false);
        terminal
            .draw(|frame| render(frame, &app))
            .expect("small frame");
        assert!(active_count(&app) == 0);
    }

    #[test]
    fn an_unusual_profile_still_renders() {
        let mut app = App::neutral(true);
        app.handle_key(Key::Char('e'));
        let text = screen(&app);
        assert!(text.contains("yolo") || text.contains("Yolo"), "{text}");
        assert!(text.contains("engineering"), "{text}");
    }
}
