//! What the terminal shows: header, band table, response plot, verdict, keys.
//!
//! The screen is deliberately quiet: one accent colour, thin rules instead of
//! boxes, letter-spaced section labels, and no key hints in any title.

use auri_codec::{Band, BandKind};
use auri_gate::{GateResult, Profile};
use auri_policy::{
    FLASH_NOTICE_TEXT, ItemId, JournalEntry, JournalStatus, LOCKED_TEXT, LOUDNESS_WARNING_TEXT,
    LoudnessReason, journal_timestamp,
};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::presets;
use crate::tui::app::{App, Col, Focus};
use crate::tui::icons::Icons;
use crate::tui::plot;
use crate::tui::theme::Theme;

/// The number of reason lines the verdict can show before it truncates.
const MAX_REASON_LINES: usize = 4;

/// The number of waived reasons shown before truncating.
const MAX_WAIVED_LINES: usize = 2;

/// The width of the gain bar, in cells.
const BAR_WIDTH: usize = 11;

/// The full scale of the gain bar, in dB.
const BAR_FULL_SCALE_DB: f64 = 12.0;

/// The width of the response gutter, where the dB labels live.
const GUTTER: usize = 5;

/// Below this width the curve cannot be drawn: the braille bitmap is used.
const CURVE_MIN_WIDTH: usize = 24;

/// Below this height the curve cannot be drawn: the braille bitmap is used.
const CURVE_MIN_HEIGHT: usize = 8;

/// The width of the band table, in cells.
const TABLE_WIDTH: usize = 46;

/// The keys, in the order they matter.
const KEY_HINTS: &str = "hjkl move · L log · +- step · c kind · x off · { } preset · t yolo · e layer · w write · q quit";

/// The keys of the journal log, in the order they matter.
const LOG_KEY_HINTS: &str = "j/k scroll · g top · G bottom · Esc or q close";

/// The width of the timestamp column, in cells (`YYYY-MM-DD HH:MM:SS`).
const LOG_TIME_WIDTH: usize = 19;

/// The width of the status column, in cells (`Interrupted` is the longest).
const LOG_STATUS_WIDTH: usize = 11;

/// The width of the CRC column, in cells (`0x` plus the hex digest).
const LOG_CRC_WIDTH: usize = 10;

/// Renders one frame.
pub fn render(frame: &mut Frame, app: &App) {
    let icons = Icons::for_mode(app.ascii);
    let theme = Theme::new();
    let area = frame.area();

    // The journal log takes over the page while it is open: the editor is
    // still there underneath, one keypress away.
    if app.log_open {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2),
                Constraint::Min(6),
                Constraint::Length(2),
            ])
            .split(area);
        header(frame, app, rows[0], &icons, &theme);
        render_log(frame, app, rows[1], &icons, &theme);
        footer(frame, app, rows[2], &theme);
        return;
    }

    let gate = gate_lines(app, &icons, &theme);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(6),
            Constraint::Length(gate_height(gate.len(), area.height)),
            Constraint::Length(2),
        ])
        .split(area);

    header(frame, app, rows[0], &icons, &theme);
    body(frame, app, rows[1], &icons, &theme);
    verdict(frame, rows[2], &gate, &icons, &theme);
    footer(frame, app, rows[3], &theme);
}

/// How many rows the gate block takes: a rule, then the verdict itself.
fn gate_height(lines: usize, height: u16) -> u16 {
    // The header, the body and the footer need room too.
    (lines as u16 + 1).min(height.saturating_sub(10))
}

fn header(frame: &mut Frame, app: &App, area: Rect, icons: &Icons, theme: &Theme) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let title = Line::from(vec![
        Span::styled(icons.note, theme.accent_style()),
        Span::styled(" kuba auri", theme.text_style()),
        Span::styled(" · CB5100", theme.structure_style()),
    ]);
    let meta = header_meta(app, theme);
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(meta.width() as u16)])
        .split(area);
    frame.render_widget(Paragraph::new(title), columns[0]);
    frame.render_widget(Paragraph::new(meta), columns[1]);

    // A thin rule under the header, instead of a box around it.
    let rule = Rect {
        y: area.y + 1,
        height: 1,
        ..area
    };
    frame.render_widget(
        Paragraph::new(Span::styled(
            icons.rule.to_string().repeat(area.width as usize),
            theme.structure_style(),
        )),
        rule,
    );
}

/// The right-hand side of the header: what is judging the curve, and how.
fn header_meta(app: &App, theme: &Theme) -> Line<'static> {
    let layer = if app.experimental {
        "engineering"
    } else {
        "ajustar"
    };
    let mut spans = vec![
        Span::styled(app.profile.as_str().to_string(), theme.text_style()),
        Span::styled(format!(" · {layer}"), theme.structure_style()),
    ];
    // A read-only marker (like eqtui's): the curve is an official preset,
    // verbatim. It is cosmetic — writing is gated exactly as for any curve.
    if let Some(preset) = app
        .preset_index
        .and_then(|index| presets::PRESETS.get(index))
    {
        spans.push(Span::styled(
            format!(" · {}", preset.label),
            theme.text_style(),
        ));
        spans.push(Span::styled(" [RO]", theme.structure_style()));
    }
    spans.push(Span::styled(" · preamp ", theme.structure_style()));
    spans.push(Span::styled(
        format!("{:+.1} dB", app.preamp_db),
        theme.text_style(),
    ));
    if app.armed {
        spans.push(Span::styled(
            " · ARMED",
            Style::default().fg(theme.warn).add_modifier(Modifier::BOLD),
        ));
    }
    Line::from(spans)
}

fn body(frame: &mut Frame, app: &App, area: Rect, icons: &Icons, theme: &Theme) {
    // The band table needs its own width: on a narrow terminal it would be
    // cut in half, so it keeps the width it needs and the response plot gets
    // what is left.
    let wanted = area.width.saturating_mul(2) / 5;
    // The table plus the hairline that separates it from the plot.
    let needed = (TABLE_WIDTH as u16 + 1).min(area.width.saturating_sub(24));
    let width = wanted.max(needed);
    let columns = [
        Rect { width, ..area },
        Rect {
            x: area.x + width,
            width: area.width.saturating_sub(width),
            ..area
        },
    ];
    // A hairline between the two panels: the columns read as one table with
    // a curve beside it, not as two boxes.
    if columns[0].width > 1 {
        let line = Line::from(Span::styled(
            icons.vertical.to_string(),
            theme.structure_style(),
        ));
        let rule: Vec<Line<'static>> = vec![line; area.height as usize];
        frame.render_widget(
            Paragraph::new(rule),
            Rect {
                x: columns[0].x + columns[0].width - 1,
                y: area.y,
                width: 1,
                height: area.height,
            },
        );
    }
    let table = Rect {
        width: columns[0].width.saturating_sub(1),
        ..columns[0]
    };
    bands(frame, app, table, icons, theme);
    response(frame, app, columns[1], icons, theme);
}

/// The journal log: what the cable was told, and when.
///
/// It is read-only by construction: it shows what the journal already says
/// and writes nothing, ever.
fn render_log(frame: &mut Frame, app: &App, area: Rect, icons: &Icons, theme: &Theme) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let label = Line::from(vec![
        Span::styled(icons.log, theme.structure_style()),
        Span::styled(" J O U R N A L", theme.label_style()),
        Span::styled(
            format!(" · {} entries · read-only", app.log_entries.len()),
            theme.structure_style(),
        ),
    ]);
    frame.render_widget(Paragraph::new(label), Rect { height: 1, ..area });
    if area.height < 2 {
        return;
    }
    frame.render_widget(
        Paragraph::new(log_header(theme)),
        Rect {
            y: area.y + 1,
            height: 1,
            ..area
        },
    );
    if area.height < 3 {
        return;
    }

    let rows = area.height.saturating_sub(2) as usize;
    let total = app.log_entries.len();
    if total == 0 {
        frame.render_widget(
            Paragraph::new(log_empty(theme)),
            Rect {
                y: area.y + 2,
                height: 1,
                ..area
            },
        );
        return;
    }

    // The view follows the row the log is on, and it never leaves the entries
    // there are.
    let selected = usize::from(app.log_scroll).min(total - 1);
    let first = selected
        .saturating_sub(rows.saturating_sub(1))
        .min(total.saturating_sub(rows));
    let mut lines = Vec::with_capacity(rows);
    for (offset, entry) in app.log_entries[first..].iter().take(rows).enumerate() {
        lines.push(log_row(entry, first + offset == selected, theme));
    }
    frame.render_widget(
        Paragraph::new(lines),
        Rect {
            y: area.y + 2,
            height: rows as u16,
            ..area
        },
    );
}

/// The column headings of the journal log.
fn log_header(theme: &Theme) -> Line<'static> {
    Line::from(vec![
        Span::raw("  "),
        Span::styled(
            format!("{:<time$}", "TIMESTAMP", time = LOG_TIME_WIDTH),
            theme.structure_style(),
        ),
        Span::raw("  "),
        Span::styled(
            format!("{:<status$}", "STATUS", status = LOG_STATUS_WIDTH),
            theme.structure_style(),
        ),
        Span::raw("  "),
        Span::styled(
            format!("{:<crc$}", "CRC", crc = LOG_CRC_WIDTH),
            theme.structure_style(),
        ),
        Span::raw("  "),
        Span::styled("LABEL", theme.structure_style()),
    ])
}

/// One row of the journal log: when it happened, how it ended, what it was.
fn log_row(entry: &JournalEntry, selected: bool, theme: &Theme) -> Line<'static> {
    let emphasised = |base: Style| {
        if selected {
            base.add_modifier(Modifier::BOLD)
        } else {
            base
        }
    };
    Line::from(vec![
        Span::raw("  "),
        Span::styled(
            format!(
                "{:<time$}",
                journal_timestamp(entry.ts),
                time = LOG_TIME_WIDTH
            ),
            emphasised(theme.text_style()),
        ),
        Span::raw("  "),
        Span::styled(
            format!(
                "{:<status$}",
                log_status_text(entry.status),
                status = LOG_STATUS_WIDTH
            ),
            emphasised(log_status_style(entry.status, theme)),
        ),
        Span::raw("  "),
        Span::styled(
            format!("{:<crc$}", entry.crc, crc = LOG_CRC_WIDTH),
            emphasised(theme.text_style()),
        ),
        Span::raw("  "),
        Span::styled(entry.label.clone(), emphasised(theme.text_style())),
    ])
}

/// How a journal status reads in the log.
const fn log_status_text(status: JournalStatus) -> &'static str {
    match status {
        JournalStatus::Ok => "ok",
        JournalStatus::Interrupted => "Interrupted",
        JournalStatus::Simulated => "Simulated",
    }
}

/// The colour of a journal status: what it says about the cable.
fn log_status_style(status: JournalStatus, theme: &Theme) -> Style {
    match status {
        JournalStatus::Ok => Style::default().fg(theme.log_ok),
        JournalStatus::Interrupted => Style::default().fg(theme.log_interrupted),
        JournalStatus::Simulated => Style::default().fg(theme.log_simulated),
    }
}

/// The empty journal: nothing to show, and nothing to worry about.
fn log_empty(theme: &Theme) -> Line<'static> {
    Line::from(Span::styled(
        "  no journal entries to show — the journal is empty or unreadable",
        theme.structure_style(),
    ))
}

fn bands(frame: &mut Frame, app: &App, area: Rect, icons: &Icons, theme: &Theme) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let label = Line::from(vec![
        Span::styled(icons.list, theme.structure_style()),
        Span::styled(" B A N D S", theme.label_style()),
        Span::styled(
            format!(" · {}/10 active", active_count(app)),
            theme.structure_style(),
        ),
    ]);
    frame.render_widget(Paragraph::new(label), Rect { height: 1, ..area });
    if area.height < 2 {
        return;
    }
    frame.render_widget(
        Paragraph::new(table_header(theme)),
        Rect {
            y: area.y + 1,
            height: 1,
            ..area
        },
    );

    let rows = area.height.saturating_sub(2) as usize;
    let mut lines = Vec::with_capacity(rows);
    for (index, band) in app.bands.iter().enumerate() {
        if lines.len() >= rows {
            break;
        }
        lines.push(band_row(app, index, band, icons, theme));
    }
    frame.render_widget(
        Paragraph::new(lines),
        Rect {
            y: area.y + 2,
            height: rows as u16,
            ..area
        },
    );
}

/// The column headings of the band table, aligned with the values below.
fn table_header(theme: &Theme) -> Line<'static> {
    Line::from(vec![
        Span::raw("  "),
        Span::styled(" # ", theme.structure_style()),
        Span::styled("KIND", theme.structure_style()),
        Span::styled("      FREQ", theme.structure_style()),
        Span::styled("      GAIN", theme.structure_style()),
        Span::styled("     Q", theme.structure_style()),
        Span::styled("        BAR", theme.structure_style()),
    ])
}

/// One row of the band table.
fn band_row(app: &App, index: usize, band: &Band, icons: &Icons, theme: &Theme) -> Line<'static> {
    let selected = index == app.row;
    let mut spans = vec![Span::raw("  ")];
    spans.push(cell(
        format!("{:>2} ", index + 1),
        selected,
        false,
        Style::default().fg(if selected {
            theme.accent
        } else {
            theme.structure
        }),
        theme,
    ));
    spans.push(cell(
        format!("{:<3} ", band.kind().as_str()),
        selected,
        selected && app.col == Col::Kind,
        theme.text_style(),
        theme,
    ));
    spans.push(cell(
        freq_text(band),
        selected,
        selected && app.col == Col::Freq,
        theme.text_style(),
        theme,
    ));
    spans.push(cell(
        gain_text(band),
        selected,
        selected && app.col == Col::Gain,
        gain_style(band, theme),
        theme,
    ));
    spans.push(cell(
        q_text(band),
        selected,
        selected && app.col == Col::Q,
        theme.text_style(),
        theme,
    ));
    spans.extend(gain_bar(band, icons, theme));
    Line::from(spans)
}

/// One table cell: the cursor paints it, the selected row emphasises it.
fn cell(text: String, row: bool, cursor: bool, style: Style, theme: &Theme) -> Span<'static> {
    if cursor {
        Span::styled(
            text,
            Style::default()
                .fg(Color::Black)
                .bg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )
    } else if row {
        Span::styled(text, style.add_modifier(Modifier::BOLD))
    } else {
        Span::styled(text, style)
    }
}

fn freq_text(band: &Band) -> String {
    match band.freq_hz() {
        Some(hz) if hz >= 1_000.0 => format!("{:>8.2}k ", hz / 1_000.0),
        Some(hz) => format!("{hz:>7.1}Hz "),
        None => "        — ".to_string(),
    }
}

fn gain_text(band: &Band) -> String {
    if band.kind().has_gain() {
        format!("{:>+7.1}dB ", band.gain_db())
    } else {
        "        — ".to_string()
    }
}

fn q_text(band: &Band) -> String {
    match band.q_factor() {
        Some(q) => format!("{q:>5.2} "),
        None => "    — ".to_string(),
    }
}

/// The colour of a gain: a boost is warm, a cut is cold.
fn gain_style(band: &Band, theme: &Theme) -> Style {
    if band.gain_db() > 0.0 {
        Style::default().fg(theme.boost)
    } else if band.gain_db() < 0.0 {
        Style::default().fg(theme.cut)
    } else {
        theme.text_style()
    }
}

/// The gain bar: centred on 0 dB, full scale at ±12 dB.
///
/// A boost fills to the right of the centre, a cut to the left; the colour
/// says which as well. A band that takes no gain shows an empty track.
fn gain_bar(band: &Band, icons: &Icons, theme: &Theme) -> Vec<Span<'static>> {
    let mut cells: Vec<(char, Style)> = Vec::with_capacity(BAR_WIDTH);
    let gain = band.gain_db();
    let usable = band.kind() != BandKind::Off && band.kind().has_gain();
    if !usable {
        for _ in 0..BAR_WIDTH {
            cells.push((icons.bar_track, theme.structure_style()));
        }
        return runs(cells);
    }

    let centre = BAR_WIDTH / 2;
    let mut chars = vec![icons.bar_track; BAR_WIDTH];
    chars[centre] = icons.bar_zero;
    let filled = ((gain.abs() / BAR_FULL_SCALE_DB) * centre as f64).round() as usize;
    let filled = filled.min(centre);
    let colour = if gain > 0.0 {
        theme.boost
    } else if gain < 0.0 {
        theme.cut
    } else {
        theme.text
    };
    let mut marks = [false; BAR_WIDTH];
    if gain > 0.0 {
        for slot in marks.iter_mut().skip(centre + 1).take(filled) {
            *slot = true;
        }
    } else if gain < 0.0 {
        for slot in marks.iter_mut().skip(centre - filled).take(filled) {
            *slot = true;
        }
    }
    for (index, ch) in chars.into_iter().enumerate() {
        if marks[index] {
            cells.push((icons.bar_fill, Style::default().fg(colour)));
        } else {
            cells.push((ch, theme.structure_style()));
        }
    }
    runs(cells)
}

fn response(frame: &mut Frame, app: &App, area: Rect, icons: &Icons, theme: &Theme) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let label = Line::from(vec![
        Span::styled(icons.wave, theme.structure_style()),
        Span::styled(" R E S P O N S E", theme.label_style()),
    ]);
    frame.render_widget(Paragraph::new(label), Rect { height: 1, ..area });
    if area.height < 4 {
        return;
    }

    // The label, the baseline and the frequency labels take three rows.
    let height = area.height.saturating_sub(3) as usize;
    let width = area.width.saturating_sub(GUTTER as u16) as usize;
    if width == 0 || height == 0 {
        return;
    }
    let samples = plot::sample(&app.bands, app.preamp_db, width);
    let values: Vec<f64> = samples.iter().map(|&(_, db)| db).collect();
    let (lo, hi) = plot::window(&values);

    let rows = if width >= CURVE_MIN_WIDTH && height >= CURVE_MIN_HEIGHT {
        curve_rows(app, lo, hi, width, height, icons, theme)
    } else {
        braille_rows(&values, lo, hi, width, height, theme)
    };
    frame.render_widget(
        Paragraph::new(rows),
        Rect {
            y: area.y + 1,
            height: height as u16,
            ..area
        },
    );

    // The baseline, with a tick on every decade mark.
    let mut baseline = vec![' '; width];
    for ch in baseline.iter_mut() {
        *ch = icons.rule;
    }
    for hz in [100.0, 1_000.0, 10_000.0] {
        let column = plot::column_for(hz, width);
        if let Some(slot) = baseline.get_mut(column) {
            *slot = icons.vertical;
        }
    }
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw(" ".repeat(GUTTER)),
            Span::styled(
                baseline.into_iter().collect::<String>(),
                theme.structure_style(),
            ),
        ])),
        Rect {
            y: area.y + 1 + height as u16,
            height: 1,
            ..area
        },
    );

    // The frequency labels, under the axis.
    let labels = place_labels(width, &frequency_labels(width));
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw(" ".repeat(GUTTER)),
            Span::styled(
                labels.into_iter().collect::<String>(),
                theme.structure_style(),
            ),
        ])),
        Rect {
            y: area.y + 2 + height as u16,
            height: 1,
            ..area
        },
    );
}

/// The plot, drawn as a line with a node on every band.
fn curve_rows(
    app: &App,
    lo: f64,
    hi: f64,
    width: usize,
    height: usize,
    icons: &Icons,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let art = plot::canvas(&plot::Response {
        bands: &app.bands,
        preamp_db: app.preamp_db,
        width,
        height,
        lo,
        hi,
        selected: Some(app.row),
        icons,
    });
    let labels = plot::y_labels(lo, hi, height);
    let mut rows = Vec::with_capacity(height);
    for row in 0..height {
        let mut cells: Vec<(char, Style)> = Vec::with_capacity(width);
        for column in 0..width {
            let mark = art.at(column, row);
            let style = match mark.ink {
                plot::Ink::Blank => Style::default(),
                plot::Ink::Grid => theme.structure_style(),
                plot::Ink::Curve => theme.accent_style(),
                plot::Ink::Node => theme.structure_style(),
                plot::Ink::NodeSelected => theme.accent_style(),
            };
            cells.push((mark.ch, style));
        }
        rows.push(Line::from(
            [
                Span::styled(y_gutter(row, &labels), theme.structure_style()),
                Span::raw(" "),
            ]
            .into_iter()
            .chain(runs(cells))
            .collect::<Vec<_>>(),
        ));
    }
    rows
}

/// The cramped fallback: the old braille bitmap.
fn braille_rows(
    values: &[f64],
    lo: f64,
    hi: f64,
    width: usize,
    height: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let art = plot::braille(values, width, height, lo, hi);
    let labels = plot::y_labels(lo, hi, height);
    let mut rows = Vec::with_capacity(height);
    for (row, text) in art.rows().into_iter().enumerate() {
        rows.push(Line::from(vec![
            Span::styled(y_gutter(row, &labels), theme.structure_style()),
            Span::raw(" "),
            Span::styled(text, theme.accent_style()),
        ]));
    }
    rows
}

/// The gutter of one row: the dB label, or nothing.
fn y_gutter(row: usize, labels: &[(usize, String)]) -> String {
    for (line, label) in labels {
        if *line == row {
            return format!("{label:>4} ");
        }
    }
    "     ".to_string()
}

/// The frequency labels of the x axis, with the column they sit on.
fn frequency_labels(width: usize) -> Vec<(usize, String)> {
    [
        (plot::PLOT_MIN_HZ, "20"),
        (100.0, "100"),
        (1_000.0, "1k"),
        (10_000.0, "10k"),
        (plot::PLOT_MAX_HZ, "20k"),
    ]
    .into_iter()
    .map(|(hz, text)| (plot::column_for(hz, width), text.to_string()))
    .collect()
}

/// Puts the frequency labels on their columns: the two ends of the axis
/// first, then the ones in between that still fit.
fn place_labels(width: usize, labels: &[(usize, String)]) -> Vec<char> {
    let mut cells = vec![' '; width];
    let count = labels.len();
    let mut order: Vec<usize> = Vec::with_capacity(count);
    if count > 0 {
        order.push(0);
        order.push(count - 1);
    }
    order.extend(1..count.saturating_sub(1));

    for index in order {
        let (column, text) = &labels[index];
        let len = text.chars().count();
        let start = match index {
            0 => *column,
            _ if index + 1 == count => column.saturating_sub(len.saturating_sub(1)),
            _ => column.saturating_sub(len / 2),
        };
        let fits =
            (start..start + len).all(|slot| cells.get(slot).is_some_and(|cell| *cell == ' '));
        if !fits {
            continue;
        }
        for (offset, ch) in text.chars().enumerate() {
            if let Some(slot) = cells.get_mut(start + offset) {
                *slot = ch;
            }
        }
    }
    cells
}

fn verdict(frame: &mut Frame, area: Rect, lines: &[Line<'static>], icons: &Icons, theme: &Theme) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    // A thin rule above the block, instead of a box around it.
    frame.render_widget(
        Paragraph::new(Span::styled(
            icons.rule.to_string().repeat(area.width as usize),
            theme.structure_style(),
        )),
        Rect { height: 1, ..area },
    );
    if area.height < 2 {
        return;
    }
    frame.render_widget(
        Paragraph::new(lines.to_vec()),
        Rect {
            y: area.y + 1,
            height: area.height - 1,
            ..area
        },
    );
}

/// The lines of the gate block: the verdict, the reasons, the checklist.
fn gate_lines(app: &App, icons: &Icons, theme: &Theme) -> Vec<Line<'static>> {
    let mut lines = Vec::with_capacity(MAX_REASON_LINES + 8);

    lines.push(status_line(&app.verdict, app.profile, theme));

    for reason in app.verdict.reasons.iter().take(MAX_REASON_LINES) {
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {} ", icons.cross),
                Style::default().fg(theme.refused),
            ),
            Span::styled(crate::describe(reason), Style::default().fg(theme.refused)),
        ]));
    }
    if app.verdict.reasons.len() > MAX_REASON_LINES {
        lines.push(Line::from(Span::styled(
            format!(
                "  … and {} more",
                app.verdict.reasons.len() - MAX_REASON_LINES
            ),
            theme.structure_style(),
        )));
    }
    if let Some(waived) = &app.verdict.waived_reasons {
        let shown = waived.len().min(MAX_WAIVED_LINES);
        for reason in waived.iter().take(shown) {
            lines.push(Line::from(vec![
                Span::styled(format!("  {} ", icons.dot), theme.structure_style()),
                Span::styled(crate::describe(reason), theme.structure_style()),
            ]));
        }
        if waived.len() > shown {
            lines.push(Line::from(Span::styled(
                format!("  … and {} waived", waived.len() - shown),
                theme.structure_style(),
            )));
        }
    }

    lines.push(level_line(app, theme));
    if app.checklist.loudness.warn {
        lines.push(Line::from(Span::styled(
            format!("  {} {LOUDNESS_WARNING_TEXT}", icons.warn),
            Style::default().fg(theme.warn),
        )));
    }
    lines.push(confirmations_line(app, icons, theme));
    if app.checklist.gate_ok && !app.checklist.can_write {
        lines.push(Line::from(Span::styled(
            format!("  {LOCKED_TEXT}"),
            theme.structure_style(),
        )));
    }
    lines.push(counters_line(app, icons, theme));
    if app.checklist.daily_notice {
        lines.push(Line::from(Span::styled(
            format!("  {} {FLASH_NOTICE_TEXT}", icons.warn),
            Style::default().fg(theme.warn),
        )));
    }

    lines
}

fn status_line(result: &GateResult, profile: Profile, theme: &Theme) -> Line<'static> {
    let (label, colour) = if result.ok {
        ("PASS", theme.pass)
    } else {
        ("REFUSED", theme.refused)
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
            theme.structure_style(),
        ),
        Span::styled("CRC ", theme.structure_style()),
        Span::styled(crc, theme.text_style()),
        Span::styled(
            format!("  {blocking} blocking"),
            if blocking == 0 {
                theme.structure_style()
            } else {
                Style::default().fg(theme.refused)
            },
        ),
    ])
}

/// The predicted level of this curve, what the last write left behind and
/// the jump between them. Computed data, not audio.
fn level_line(app: &App, theme: &Theme) -> Line<'static> {
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
        theme.warn
    } else {
        theme.structure
    };

    Line::from(vec![
        Span::styled("  level 300 Hz–3 kHz ", theme.structure_style()),
        Span::styled(
            level,
            Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
        ),
        Span::styled("   last written ", theme.structure_style()),
        Span::styled(last, theme.structure_style()),
        Span::styled("   jump ", theme.structure_style()),
        Span::styled(jump, Style::default().fg(jump_colour)),
    ])
}

/// The tickable confirmations on one line: a box when satisfied, an empty one
/// when not, and an arrow on the row the checklist cursor holds.
fn confirmations_line(app: &App, icons: &Icons, theme: &Theme) -> Line<'static> {
    let visible = app.visible_confirmations();
    let mut spans = vec![Span::raw("  ")];
    if visible.is_empty() {
        spans.push(Span::styled("nothing to confirm", theme.structure_style()));
    }
    for (index, id) in visible.iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled("   ", Style::default()));
        }
        if app.focus == Focus::Checklist && index == app.confirm_row {
            spans.push(Span::styled(
                format!("{} ", icons.arrow),
                theme.structure_style(),
            ));
        }
        let checked = app.checklist.item(*id).is_some_and(|item| item.satisfied);
        let style = if checked {
            theme.text_style()
        } else {
            theme.structure_style()
        };
        spans.push(Span::styled(
            format!(
                "{} {}",
                if checked {
                    icons.checked
                } else {
                    icons.unchecked
                },
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
fn counters_line(app: &App, icons: &Icons, theme: &Theme) -> Line<'static> {
    let counters = app.checklist.counters;
    let spacing = app.checklist.spacing;
    let wait = if spacing.ok {
        icons.tick.to_string()
    } else {
        format!("{:.1}s", spacing.wait_ms as f64 / 1000.0)
    };
    let wait_style = if spacing.ok {
        theme.structure_style()
    } else {
        Style::default().fg(theme.warn)
    };

    Line::from(vec![
        Span::styled(
            format!(
                "  {} this session · {} today · {} in total",
                counters.session, counters.day, counters.total
            ),
            theme.structure_style(),
        ),
        Span::styled("   spacing ", theme.structure_style()),
        Span::styled(wait, wait_style),
        Span::styled("   to ", theme.structure_style()),
        Span::styled(app.target.clone(), theme.text_style()),
    ])
}

fn footer(frame: &mut Frame, app: &App, area: Rect, theme: &Theme) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    frame.render_widget(
        Paragraph::new(Span::styled(app.status.clone(), theme.text_style())),
        Rect { height: 1, ..area },
    );
    if area.height < 2 {
        return;
    }
    // The log overlay has its own keys: the editor's are not its own.
    let hints = if app.log_open {
        LOG_KEY_HINTS
    } else {
        KEY_HINTS
    };
    frame.render_widget(
        Paragraph::new(Span::styled(hints, theme.structure_style())),
        Rect {
            y: area.y + 1,
            height: 1,
            ..area
        },
    );
}

/// Merges neighbouring cells that share a style into single spans.
fn runs(cells: Vec<(char, Style)>) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut text = String::new();
    let mut style: Option<Style> = None;
    for (ch, cell_style) in cells {
        if style != Some(cell_style) {
            if let Some(previous) = style {
                spans.push(Span::styled(std::mem::take(&mut text), previous));
            }
            style = Some(cell_style);
        }
        text.push(ch);
    }
    if let Some(previous) = style {
        spans.push(Span::styled(text, previous));
    }
    spans
}

fn active_count(app: &App) -> usize {
    app.bands
        .iter()
        .filter(|band| band.kind() != BandKind::Off)
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::Key;
    use auri_codec::NEUTRAL_BANDS;
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

    /// One journaled write attempt, as the log would show it.
    fn journal_entry(ts: i64, status: JournalStatus, crc: &str, label: &str) -> JournalEntry {
        JournalEntry {
            ts,
            status,
            crc: crc.to_string(),
            bands: NEUTRAL_BANDS.to_vec(),
            preamp_db: -0.5,
            profile: Profile::Standard,
            label: label.to_string(),
        }
    }

    /// An editor with the log overlay open and the given entries in it.
    fn with_log(entries: Vec<JournalEntry>) -> App {
        let mut app = App::neutral(false);
        app.log_open = true;
        app.log_entries = entries;
        app
    }

    #[test]
    fn a_frame_renders_the_header_table_plot_and_keys() {
        let text = screen(&App::neutral(false));
        assert!(text.contains("kuba auri"), "header missing");
        assert!(text.contains("gate: PASS"), "verdict missing: {text}");
        assert!(text.contains("R E S P O N S E"), "plot missing: {text}");
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
        assert!(
            !screen(&App::neutral(false)).contains("[RO]"),
            "file curve: no marker"
        );
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
        assert!(focused.contains("☑"), "ticked items: {focused}");
        assert!(!focused.contains("Complete every item"), "{focused}");
    }

    /// Every character cell that carries the cursor background.
    ///
    /// Only the cursor paints an *accent* background — the plot and the
    /// header only tint their foreground — so this cannot pick up anything
    /// else. Note that a table cell is several characters wide, which is why
    /// this returns a list rather than a count of "cells".
    fn yellow_spots(app: &App) -> Vec<(u16, u16)> {
        let mut terminal = terminal();
        terminal.draw(|frame| render(frame, app)).expect("draw");
        let buffer = terminal.backend().buffer();
        (0..buffer.area.height)
            .flat_map(|y| (0..buffer.area.width).map(move |x| (x, y)))
            .filter(|&(x, y)| buffer[(x, y)].bg == Theme::new().accent)
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

    /// The gain column shows the sign it means to show.
    ///
    /// `{:+>8.1}` is a trap: Rust reads the fill character and the alignment
    /// *before* the sign, so `+` became the fill and the sign was dropped —
    /// a positive gain rendered as `+++++0.5` and a negative one as
    /// `++++-0.5`. The sign flag has to come first: `{:>+8.1}`.
    #[test]
    fn the_gain_column_keeps_its_sign() {
        let mut app = App::neutral(false);
        app.handle_key(Key::Char('c')); // band 1 → PK
        app.handle_key(Key::Char('l')); // kind → freq
        app.handle_key(Key::Char('l')); // freq → gain
        app.handle_key(Key::Char('+')); // +0.5 dB

        let text = screen(&app);
        assert!(text.contains("+0.5dB"), "the sign must survive: {text}");
        assert!(
            !text.contains("++++"),
            "the fill must not be a row of plus signs: {text}"
        );

        app.handle_key(Key::Char('-'));
        app.handle_key(Key::Char('-')); // -0.5 dB
        let text = screen(&app);
        assert!(
            text.contains("-0.5dB"),
            "a negative gain keeps its sign: {text}"
        );
        assert!(
            !text.contains("++++"),
            "the fill must not be a row of plus signs: {text}"
        );
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

    #[test]
    fn the_ascii_mode_draws_no_glyph_at_all() {
        let mut app = ready();
        app.ascii = true;
        let text = screen(&app);
        for ch in text.chars() {
            assert!(!is_glyph(ch), "the ascii mode must not draw {ch:?}: {text}");
        }
        assert!(text.contains("[x]"), "the confirmations are ascii: {text}");
        assert!(text.contains("B A N D S"), "the label survives: {text}");
    }

    /// Whether a character belongs to one of the blocks the glyphs come from:
    /// box drawing, braille, geometric shapes, symbols, dingbats and arrows.
    fn is_glyph(ch: char) -> bool {
        matches!(ch as u32,
            0x2190..=0x21FF | 0x223F | 0x2261 | 0x2500..=0x259F
            | 0x25A0..=0x25FF | 0x2600..=0x27BF | 0x2800..=0x28FF)
    }

    #[test]
    fn the_gain_bar_is_centred_on_zero_and_fills_towards_the_sign() {
        let icons = Icons::unicode();
        let theme = Theme::new();
        let boost = Band::Pk {
            freq_hz: 1_000.0,
            gain_db: 6.0,
            q_factor: 1.0,
        };
        let cut = Band::Pk {
            freq_hz: 1_000.0,
            gain_db: -6.0,
            q_factor: 1.0,
        };
        let boost_text = bar_text(&gain_bar(&boost, &icons, &theme));
        let cut_text = bar_text(&gain_bar(&cut, &icons, &theme));
        assert_eq!(boost_text.chars().count(), BAR_WIDTH);
        assert_eq!(cut_text.chars().count(), BAR_WIDTH);
        // The centre is the zero, and the fill sits on the correct side.
        assert_eq!(
            boost_text.chars().position(|ch| ch == icons.bar_fill),
            Some(6),
            "a boost fills to the right: {boost_text}"
        );
        assert_eq!(
            cut_text.chars().position(|ch| ch == icons.bar_fill),
            Some(2),
            "a cut fills to the left: {cut_text}"
        );
        assert!(
            boost_text.contains('│') && cut_text.contains('│'),
            "the zero is marked: {boost_text}"
        );
    }

    /// The text of a bar, whatever the runs it is made of.
    fn bar_text(spans: &[Span<'_>]) -> String {
        spans.iter().map(|span| span.content.as_ref()).collect()
    }

    #[test]
    fn an_off_band_shows_an_empty_gain_bar() {
        let icons = Icons::unicode();
        let theme = Theme::new();
        let bar = gain_bar(&Band::Off, &icons, &theme);
        let text: String = bar.iter().map(|span| span.content.as_ref()).collect();
        assert_eq!(text.chars().count(), BAR_WIDTH);
        assert!(
            !text.contains(icons.bar_fill),
            "an off band fills nothing: {text}"
        );
        assert!(!text.contains('│'), "an off band has no zero mark: {text}");
    }

    #[test]
    fn the_frequency_labels_sit_on_their_columns() {
        let labels = frequency_labels(66);
        assert_eq!(labels[0].0, 0);
        assert_eq!(labels[4].0, 65);
        let cells = place_labels(66, &labels);
        let text: String = cells.into_iter().collect();
        assert!(text.starts_with("20"), "{text}");
        assert!(text.ends_with("20k"), "{text}");
        assert!(text.contains("100"), "{text}");
        assert!(text.contains("1k"), "{text}");
    }

    #[test]
    fn a_log_view_renders_journal_entries() {
        let app = with_log(vec![
            journal_entry(1_700_000_000_000, JournalStatus::Ok, "0x4fff", "Kuba Auri"),
            journal_entry(
                1_700_000_060_000,
                JournalStatus::Interrupted,
                "0xbeef",
                "Kuba Auri",
            ),
            journal_entry(
                1_700_000_120_000,
                JournalStatus::Simulated,
                "0xcafe",
                "preview",
            ),
        ]);
        let text = screen(&app);

        assert!(
            text.contains("J O U R N A L"),
            "the log names itself: {text}"
        );
        assert!(text.contains("TIMESTAMP"), "the header row: {text}");
        assert!(text.contains("STATUS"), "the header row: {text}");
        assert!(text.contains("CRC"), "the header row: {text}");
        assert!(text.contains("LABEL"), "the header row: {text}");
        assert!(
            text.contains(&journal_timestamp(1_700_000_000_000)),
            "the timestamp must be human-readable: {text}"
        );
        assert!(text.contains("0x4fff"), "the crc is shown: {text}");
        assert!(text.contains("0xbeef"), "the crc is shown: {text}");
        assert!(text.contains("0xcafe"), "the crc is shown: {text}");
        assert!(text.contains("ok"), "the status is shown: {text}");
        assert!(text.contains("Interrupted"), "the status is shown: {text}");
        assert!(text.contains("Simulated"), "the status is shown: {text}");
        assert!(text.contains("Kuba Auri"), "the label is shown: {text}");
        assert!(text.contains("preview"), "the label is shown: {text}");
        assert!(
            text.contains("read-only"),
            "the log says what it is: {text}"
        );
    }

    #[test]
    fn a_log_view_handles_empty_journal() {
        let app = with_log(Vec::new());
        let text = screen(&app);
        assert!(
            text.contains("J O U R N A L"),
            "the log names itself: {text}"
        );
        assert!(
            text.contains("no journal entries"),
            "the empty state must say so: {text}"
        );
        assert!(
            text.contains("read-only"),
            "the log says what it is: {text}"
        );
    }

    #[test]
    fn the_log_view_takes_over_the_page() {
        let app = with_log(vec![journal_entry(
            1_700_000_000_000,
            JournalStatus::Ok,
            "0x4fff",
            "Kuba Auri",
        )]);
        let text = screen(&app);
        assert!(
            !text.contains("R E S P O N S E"),
            "the plot is gone: {text}"
        );
        assert!(!text.contains("gate:"), "the verdict is gone: {text}");
        assert!(text.contains("kuba auri"), "the header stays: {text}");
        assert!(
            text.contains("j/k scroll"),
            "the footer carries the log's keys: {text}"
        );
    }

    #[test]
    fn the_log_scrolls_within_the_entries_there_are() {
        let entries = (0..40)
            .map(|index| {
                journal_entry(
                    1_700_000_000_000 + i64::from(index) * 1_000,
                    JournalStatus::Ok,
                    "0x4fff",
                    "Kuba Auri",
                )
            })
            .collect();
        let mut app = with_log(entries);

        let first = screen(&app);
        assert!(
            first.contains(&journal_timestamp(1_700_000_000_000)),
            "the first entry is shown: {first}"
        );

        app.handle_key(Key::Char('j'));
        assert_eq!(app.log_scroll, 1);
        app.handle_key(Key::Char('G'));
        assert_eq!(app.log_scroll, 39, "the scroll stops at the last entry");
        let last = screen(&app);
        assert!(
            last.contains(&journal_timestamp(1_700_000_039_000)),
            "the last entry is shown: {last}"
        );
        assert!(
            !last.contains(&journal_timestamp(1_700_000_000_000)),
            "the view followed the scroll: {last}"
        );

        app.handle_key(Key::Char('k'));
        assert_eq!(app.log_scroll, 38);
        app.handle_key(Key::Char('g'));
        assert_eq!(app.log_scroll, 0);
    }

    #[test]
    fn rendering_the_log_never_panics_on_a_tiny_terminal() {
        let mut terminal = Terminal::new(TestBackend::new(10, 6)).expect("test backend");
        let app = with_log(vec![journal_entry(
            1_700_000_000_000,
            JournalStatus::Ok,
            "0x4fff",
            "Kuba Auri",
        )]);
        terminal
            .draw(|frame| render(frame, &app))
            .expect("small frame");
    }
}
