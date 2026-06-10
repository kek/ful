//! dum rendering: responsive activity table, glow styling, help overlay.

use std::time::Instant;

use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table};
use ratatui::Frame;

use ful::format::{human_bytes, human_rate};

use crate::activity::RING_BUCKETS;
use crate::app::App;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Col {
    Rate,
    Spark,
    Size,
    Bar,
    Name,
}

pub fn columns_for_width(width: u16) -> Vec<Col> {
    use Col::*;
    if width >= 64 {
        vec![Rate, Spark, Size, Bar, Name]
    } else if width >= 48 {
        vec![Rate, Size, Bar, Name]
    } else {
        vec![Size, Bar, Name]
    }
}

fn col_constraint(c: Col) -> Constraint {
    match c {
        Col::Rate => Constraint::Length(9),
        Col::Spark => Constraint::Length(8),
        Col::Size => Constraint::Length(5),
        Col::Bar => Constraint::Length(16),
        Col::Name => Constraint::Min(10),
    }
}

fn header_label(c: Col) -> &'static str {
    match c {
        Col::Rate => "RATE",
        Col::Spark => "LAST 30s",
        Col::Size => "SIZE",
        Col::Bar => "USAGE",
        Col::Name => "NAME",
    }
}

/// Style for a glow value: sign -> color, log-magnitude -> emphasis band.
pub fn glow_style(glow: f64) -> Style {
    let mag = glow.abs();
    if mag < 1.0 {
        return Style::default();
    }
    let color = if glow > 0.0 { Color::Green } else { Color::Red };
    let style = Style::default().fg(color);
    if mag >= 1_000_000.0 {
        style.add_modifier(Modifier::BOLD)
    } else if mag < 1_000.0 {
        style.add_modifier(Modifier::DIM)
    } else {
        style
    }
}

/// 8-char sparkline of the newest 8 ring buckets, scaled to their max |value|.
pub fn spark(ring: &[i64; RING_BUCKETS]) -> String {
    const CHARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let newest = &ring[RING_BUCKETS - 8..];
    let max = newest.iter().map(|v| v.abs()).max().unwrap_or(0);
    newest
        .iter()
        .map(|&v| {
            if max == 0 || v == 0 {
                CHARS[0]
            } else {
                let idx = ((v.abs() as f64 / max as f64) * 7.0).round() as usize;
                CHARS[idx.min(7)]
            }
        })
        .collect()
}

pub fn signed_rate(glow: f64) -> String {
    let sign = if glow >= 0.0 { "+" } else { "-" };
    format!("{}{}", sign, human_rate(glow.abs()))
}

fn usage_bar(pct: f64) -> String {
    let cells = 9usize;
    let filled = ((pct / 100.0).clamp(0.0, 1.0) * cells as f64).round() as usize;
    let mut s = String::from("[");
    for i in 0..cells {
        s.push(if i < filled.min(cells) { '█' } else { '░' });
    }
    s.push(']');
    s
}

/// Width available to the NAME column: total minus fixed columns and spacing.
fn name_width(cols: &[Col], total: u16) -> usize {
    let fixed: u16 = cols
        .iter()
        .filter(|c| !matches!(c, Col::Name))
        .map(|c| match col_constraint(*c) {
            Constraint::Length(n) => n,
            _ => 0,
        })
        .sum();
    let spacing = cols.len() as u16 - 1; // column_spacing(1)
    total.saturating_sub(fixed + spacing).max(10) as usize
}

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

pub fn draw(app: &App, frame: &mut Frame, now: Instant) {
    let area = frame.area();
    let [title_area, table_area, footer_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(area);

    draw_title(app, frame, title_area);
    draw_rows(app, frame, table_area, now);
    draw_footer(app, frame, footer_area);

    if app.show_help {
        draw_help(frame, area);
    }
}

fn draw_title(app: &App, frame: &mut Frame, area: Rect) {
    let status = if !app.watching {
        "not watching"
    } else if app.watch_degraded {
        "degraded"
    } else {
        "watching"
    };
    let total = human_bytes(app.tree.get(app.tree.root).size);
    let right = format!("{status} · total {total} ");
    let [l, r] =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(right.len() as u16 + 1)])
            .areas(area);
    let path = app.tree.path_of(app.current);
    frame.render_widget(
        Paragraph::new(Line::from(
            Span::from(format!(" dum — {}", path.display())).bold(),
        )),
        l,
    );
    frame.render_widget(Paragraph::new(right).alignment(Alignment::Right), r);
}

fn draw_rows(app: &App, frame: &mut Frame, area: Rect, now: Instant) {
    let kids = app.sorted_children(app.current);
    if kids.is_empty() {
        let msg = if app.scanning { "scanning…" } else { "empty directory" };
        frame.render_widget(
            Paragraph::new(msg).alignment(Alignment::Center),
            area,
        );
        return;
    }
    let cols = columns_for_width(area.width);
    let name_w = name_width(&cols, area.width);
    let dir_total = app.tree.get(app.current).size.max(1);
    let header = Row::new(
        cols.iter()
            .map(|c| Cell::from(header_label(*c)))
            .collect::<Vec<_>>(),
    )
    .style(Style::new().bold());

    let body: Vec<Row> = kids
        .iter()
        .enumerate()
        .map(|(i, &id)| {
            let node = app.tree.get(id);
            let glow = app.activity.glow(id, now);
            let pct = node.size as f64 / dir_total as f64 * 100.0;
            let cells: Vec<Cell> = cols
                .iter()
                .map(|c| match c {
                    Col::Rate => {
                        if glow.abs() >= 1.0 {
                            Cell::from(signed_rate(glow)).style(glow_style(glow))
                        } else {
                            Cell::from("")
                        }
                    }
                    Col::Spark => match app.activity.sparkline(id) {
                        Some(ring) => Cell::from(spark(&ring)).style(glow_style(glow)),
                        None => Cell::from(""),
                    },
                    Col::Size => Cell::from(human_bytes(node.size)),
                    Col::Bar => Cell::from(format!("{} {:>3.0}%", usage_bar(pct), pct)),
                    Col::Name => {
                        let mut label = node.name.to_string_lossy().into_owned();
                        if node.is_dir {
                            label.push('/');
                        }
                        if node.denied {
                            label.push_str(" [denied]");
                        }
                        Cell::from(truncate_ellipsis(&label, name_w)).style(glow_style(glow))
                    }
                })
                .collect();
            let row = Row::new(cells);
            if i == app.selected {
                // Subtle background + bold: highlights the row without inverting
                // the usage bar or washing out the glow colors.
                row.style(Style::new().bg(Color::DarkGray).add_modifier(Modifier::BOLD))
            } else {
                row
            }
        })
        .collect();

    let widths: Vec<Constraint> = cols.iter().map(|c| col_constraint(*c)).collect();
    let table = Table::new(body, widths).header(header).column_spacing(1);
    frame.render_widget(table, area);
}

fn draw_footer(app: &App, frame: &mut Frame, area: Rect) {
    let left = " q quit  ? help  r rescan  ⏎ enter  u up";
    let right = if app.scanning {
        format!("scanning… {} items ", app.items_seen)
    } else if let Some((dirs, files, errors)) = app.done_stats {
        if errors > 0 {
            format!("{dirs} dirs · {files} files · {errors} errors ")
        } else {
            format!("{dirs} dirs · {files} files ")
        }
    } else {
        String::new()
    };
    let [l, r] =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(right.len() as u16 + 1)])
            .areas(area);
    frame.render_widget(Paragraph::new(left).style(Style::new().dim()), l);
    frame.render_widget(
        Paragraph::new(right)
            .alignment(Alignment::Right)
            .style(Style::new().dim()),
        r,
    );
}

fn draw_help(frame: &mut Frame, area: Rect) {
    let lines = vec![
        Line::from("dum — keybindings"),
        Line::from(""),
        Line::from("  ↑↓ / jk        move selection"),
        Line::from("  ⏎ / l / →      enter directory"),
        Line::from("  u / h / ← / ⌫  go up"),
        Line::from("  r              rescan"),
        Line::from("  ?              toggle this help"),
        Line::from("  q / Esc        quit"),
        Line::from(""),
        Line::from("green = growing   red = shrinking"),
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
    use crate::scanner::{ScanEntry, ScanMsg};
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::Terminal;
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};

    fn buffer_text(buf: &Buffer) -> String {
        buf.content.iter().map(|c| c.symbol()).collect()
    }

    fn demo_app() -> App {
        let mut app = App::new(Path::new("/r"), Instant::now());
        app.apply_scan(ScanMsg::Dir {
            path: PathBuf::from("/r"),
            entries: vec![
                ScanEntry { name: OsString::from("big"), size: 0, is_dir: true },
                ScanEntry { name: OsString::from("file.txt"), size: 1024, is_dir: false },
            ],
            denied: false,
        });
        app.apply_scan(ScanMsg::Done { dirs: 1, files: 1, errors: 0 });
        app
    }

    fn render(app: &App, w: u16, h: u16) -> String {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| draw(app, f, Instant::now())).unwrap();
        buffer_text(term.backend().buffer())
    }

    #[test]
    fn column_tiers_by_width() {
        assert_eq!(columns_for_width(80), vec![Col::Rate, Col::Spark, Col::Size, Col::Bar, Col::Name]);
        assert_eq!(columns_for_width(50), vec![Col::Rate, Col::Size, Col::Bar, Col::Name]);
        assert_eq!(columns_for_width(40), vec![Col::Size, Col::Bar, Col::Name]);
    }

    #[test]
    fn glow_style_sign_and_magnitude() {
        assert_eq!(glow_style(0.0), Style::default());
        let g = glow_style(50_000.0);
        assert_eq!(g.fg, Some(Color::Green));
        let r = glow_style(-5_000_000.0);
        assert_eq!(r.fg, Some(Color::Red));
        assert!(r.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn spark_renders_eight_scaled_chars() {
        let mut ring = [0i64; RING_BUCKETS];
        ring[RING_BUCKETS - 1] = 100; // newest, max
        ring[RING_BUCKETS - 2] = 50;
        let s = spark(&ring);
        assert_eq!(s.chars().count(), 8);
        assert!(s.ends_with('█'));
    }

    #[test]
    fn signed_rate_formats_with_sign() {
        assert_eq!(signed_rate(1_258_291.0), "+1.2M/s");
        assert_eq!(signed_rate(-512.0), "-512 B/s");
    }

    #[test]
    fn full_width_render_shows_headers_and_entries() {
        let text = render(&demo_app(), 80, 12);
        assert!(text.contains("dum"));
        assert!(text.contains("RATE"));
        assert!(text.contains("LAST 30s"));
        assert!(text.contains("big"));
        assert!(text.contains("file.txt"));
    }

    #[test]
    fn narrow_render_drops_spark_and_rate() {
        let text = render(&demo_app(), 40, 12);
        assert!(!text.contains("LAST 30s"));
        assert!(!text.contains("RATE"));
        assert!(text.contains("SIZE"));
    }

    #[test]
    fn active_row_renders_rate_spark_and_full_bar() {
        let mut app = demo_app();
        let now = Instant::now();
        // Make "file.txt" the only sized entry => 100% of the dir, and active.
        let f = app.tree.lookup(Path::new("/r/file.txt")).unwrap();
        app.activity.record(f, 2_000_000, now);
        let mut term = Terminal::new(TestBackend::new(64, 12)).unwrap();
        term.draw(|fr| draw(&app, fr, now)).unwrap();
        let text = buffer_text(term.backend().buffer());
        assert!(text.contains("+")); // signed rate visible
        assert!(text.contains("█")); // bar and/or spark filled
        assert!(text.contains("100%")); // full-width percent not clipped
    }

    #[test]
    fn empty_dir_and_help_overlay() {
        let mut app = demo_app();
        app.show_help = true;
        let text = render(&app, 80, 14);
        assert!(text.contains("keybindings"));
    }
}
