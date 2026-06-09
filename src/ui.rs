//! Pure rendering: responsive table, usage bar, footer, help overlay.

use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table};
use ratatui::Frame;

use crate::app::App;
use crate::format::{human_bytes, human_rate};
use crate::model::MountRow;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Column {
    Mount,
    Fs,
    Usage,
    Used,
    Free,
    Total,
    Read,
    Write,
    Dev,
}

/// Choose which columns fit, widest set first.
pub fn columns_for_width(width: u16) -> Vec<Column> {
    use Column::*;
    if width >= 80 {
        vec![Mount, Fs, Usage, Used, Free, Total, Read, Write, Dev]
    } else if width >= 58 {
        vec![Mount, Usage, Free, Total, Read, Write]
    } else if width >= 40 {
        vec![Mount, Usage, Free]
    } else {
        vec![Mount, Usage]
    }
}

fn col_width(c: Column) -> u16 {
    match c {
        Column::Mount => 14,
        Column::Fs => 5,
        Column::Usage => 15,
        Column::Used => 5,
        Column::Free => 5,
        Column::Total => 5,
        Column::Read => 7,
        Column::Write => 7,
        Column::Dev => 7,
    }
}

fn header_label(c: Column) -> &'static str {
    match c {
        Column::Mount => "MOUNT",
        Column::Fs => "FS",
        Column::Usage => "USAGE",
        Column::Used => "USED",
        Column::Free => "FREE",
        Column::Total => "TOTL",
        Column::Read => "READ/s",
        Column::Write => "WRITE/s",
        Column::Dev => "DEV",
    }
}

/// Build the bracketed block-character usage bar (no percent).
pub fn usage_bar(pct: f64, cells: usize) -> String {
    let ratio = (pct / 100.0).clamp(0.0, 1.0);
    let filled = (ratio * cells as f64).round() as usize;
    let filled = filled.min(cells);
    let mut s = String::with_capacity(cells + 2);
    s.push('[');
    for i in 0..cells {
        s.push(if i < filled { '█' } else { '░' });
    }
    s.push(']');
    s
}

pub fn bar_color(pct: f64) -> Color {
    if pct > 90.0 {
        Color::Red
    } else if pct >= 70.0 {
        Color::Yellow
    } else {
        Color::Green
    }
}

/// Truncate to `width` chars, appending `…` when clipped.
fn truncate_ellipsis(s: &str, width: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= width {
        return s.to_string();
    }
    if width == 0 {
        return String::new();
    }
    let mut out: String = chars[..width - 1].iter().collect();
    out.push('…');
    out
}

fn io_text(rate: Option<f64>) -> String {
    match rate {
        Some(r) => human_rate(r),
        None => "—".to_string(),
    }
}

fn cell_for<'a>(r: &MountRow, c: Column) -> Cell<'a> {
    match c {
        Column::Mount => Cell::from(truncate_ellipsis(&r.mount, col_width(Column::Mount) as usize)),
        Column::Fs => Cell::from(truncate_ellipsis(&r.fs_type, col_width(Column::Fs) as usize)),
        Column::Usage => {
            let bar = usage_bar(r.used_pct, 8);
            Cell::from(Line::from(vec![
                Span::styled(bar, Style::new().fg(bar_color(r.used_pct))),
                Span::raw(format!(" {:>3.0}%", r.used_pct)),
            ]))
        }
        Column::Used => Cell::from(human_bytes(r.used)),
        Column::Free => Cell::from(human_bytes(r.free)),
        Column::Total => Cell::from(human_bytes(r.total)),
        Column::Read => Cell::from(io_text(r.read_per_s)),
        Column::Write => Cell::from(io_text(r.write_per_s)),
        Column::Dev => Cell::from(truncate_ellipsis(&r.device, col_width(Column::Dev) as usize)),
    }
}

/// Render the whole dashboard.
pub fn draw(app: &App, frame: &mut Frame) {
    let area = frame.area();
    let [title_area, table_area, footer_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(area);

    draw_title(app, frame, title_area);

    let rows = app.visible_rows();
    if app.all_rows.is_empty() {
        let msg = Paragraph::new("No filesystems found").alignment(Alignment::Center);
        frame.render_widget(msg, table_area);
    } else {
        draw_table(&rows, frame, table_area);
    }

    draw_footer(app, rows.len(), frame, footer_area);

    if app.show_help {
        draw_help(frame, area);
    }
}

fn draw_title(app: &App, frame: &mut Frame, area: Rect) {
    let right = format!("refresh {}s ", app.config.interval.as_secs());
    let [l, r] =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(right.len() as u16 + 1)])
            .areas(area);
    frame.render_widget(
        Paragraph::new(Line::from(Span::from(" ful — disk usage monitor").bold())),
        l,
    );
    frame.render_widget(Paragraph::new(right).alignment(Alignment::Right), r);
}

fn draw_table(rows: &[&MountRow], frame: &mut Frame, area: Rect) {
    let cols = columns_for_width(area.width);
    let header = Row::new(cols.iter().map(|c| Cell::from(header_label(*c))).collect::<Vec<_>>())
        .style(Style::new().bold());
    let body: Vec<Row> = rows
        .iter()
        .map(|r| Row::new(cols.iter().map(|c| cell_for(r, *c)).collect::<Vec<_>>()))
        .collect();
    let widths: Vec<Constraint> = cols
        .iter()
        .map(|c| Constraint::Length(col_width(*c)))
        .collect();
    let table = Table::new(body, widths).header(header).column_spacing(1);
    frame.render_widget(table, area);
}

fn draw_footer(app: &App, visible: usize, frame: &mut Frame, area: Rect) {
    let toggle = if app.show_all { "a hide pseudo" } else { "a toggle all" };
    let left = format!(" q quit   ? help   {toggle}");
    let right = format!("showing {} of {} mounts ", visible, app.all_rows.len());
    let [l, r] =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(right.len() as u16 + 1)])
            .areas(area);
    frame.render_widget(Paragraph::new(left).style(Style::new().dim()), l);
    frame.render_widget(
        Paragraph::new(right).alignment(Alignment::Right).style(Style::new().dim()),
        r,
    );
}

fn draw_help(frame: &mut Frame, area: Rect) {
    let lines = vec![
        Line::from("ful — keybindings"),
        Line::from(""),
        Line::from("  q / Esc   quit"),
        Line::from("  ?         toggle this help"),
        Line::from("  a         show/hide pseudo filesystems"),
        Line::from(""),
        Line::from("bar: green <70%  yellow 70-90%  red >90%"),
    ];
    let w = 46.min(area.width);
    let h = (lines.len() as u16 + 2).min(area.height);
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    let rect = Rect { x, y, width: w, height: h };
    frame.render_widget(Clear, rect);
    frame.render_widget(
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(" help ")),
        rect,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{App, Config};
    use crate::model::MountRow;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::Terminal;
    use std::time::Duration;

    fn buffer_text(buf: &Buffer) -> String {
        buf.content.iter().map(|c| c.symbol()).collect()
    }

    fn row(mount: &str, dev: &str, pct: f64) -> MountRow {
        MountRow {
            mount: mount.to_string(),
            device: dev.to_string(),
            fs_type: "apfs".to_string(),
            used: 680,
            free: 320,
            total: 1000,
            used_pct: pct,
            read_per_s: Some(1_258_291.0),
            write_per_s: None,
        }
    }

    fn app_with_rows(rows: Vec<MountRow>) -> App {
        let mut app = App::new(Config { interval: Duration::from_secs(2) });
        app.io_available = true;
        app.all_rows = rows;
        app
    }

    #[test]
    fn columns_by_width_tiers() {
        assert_eq!(columns_for_width(100).len(), 9);
        assert_eq!(columns_for_width(60).len(), 6);
        assert_eq!(columns_for_width(45).len(), 3);
        assert_eq!(columns_for_width(30).len(), 2);
    }

    #[test]
    fn usage_bar_fills_proportionally() {
        assert_eq!(usage_bar(0.0, 8), "[░░░░░░░░]");
        assert_eq!(usage_bar(100.0, 8), "[████████]");
        assert_eq!(usage_bar(50.0, 8), "[████░░░░]");
    }

    #[test]
    fn bar_color_thresholds() {
        assert_eq!(bar_color(10.0), Color::Green);
        assert_eq!(bar_color(80.0), Color::Yellow);
        assert_eq!(bar_color(95.0), Color::Red);
    }

    #[test]
    fn full_width_renders_all_headers() {
        let app = app_with_rows(vec![row("/", "disk3s1", 68.0)]);
        let backend = TestBackend::new(80, 10);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| draw(&app, f)).unwrap();
        let text = buffer_text(term.backend().buffer());
        assert!(text.contains("ful"));
        assert!(text.contains("MOUNT"));
        assert!(text.contains("WRITE/s"));
        assert!(text.contains("disk3s1"));
    }

    #[test]
    fn narrow_width_drops_io_columns() {
        let app = app_with_rows(vec![row("/", "disk3s1", 68.0)]);
        let backend = TestBackend::new(45, 10);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| draw(&app, f)).unwrap();
        let text = buffer_text(term.backend().buffer());
        assert!(text.contains("MOUNT"));
        assert!(!text.contains("WRITE/s"));
    }

    #[test]
    fn empty_shows_placeholder() {
        let app = app_with_rows(vec![]);
        let backend = TestBackend::new(80, 10);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| draw(&app, f)).unwrap();
        let text = buffer_text(term.backend().buffer());
        assert!(text.contains("No filesystems found"));
    }
}
