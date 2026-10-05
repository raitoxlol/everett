//! `everett onboard` terminal wizard (ratatui + crossterm), mirroring the Python curses TUI:
//! title bar + "step N of 7" + progress rail frame, seven screens, `q` cancels without
//! writing anything. Only renders and mutates `OnboardConfig`; the `apply_*` functions in
//! `onboard` do the real work.

use std::io;
use std::time::Duration;

use crossterm::cursor;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::{Frame, Terminal};

use crate::onboard::{self, OnboardConfig};

const STEP_TITLES: &[&str] = &[
    "Welcome",
    "Detect",
    "Hooks",
    "MCP server",
    "Smarter routing",
    "Backfill",
    "Confirm",
];
const MIN_COLS: u16 = 54;
const MIN_ROWS: u16 = 14;

enum Dir {
    Forward,
    Back,
    Apply,
}

struct Quit;

type TResult<T> = Result<T, Quit>;

struct Palette {
    ascii: bool,
    accent: Style,
    dim: Style,
    green: Style,
    amber: Style,
}

impl Palette {
    fn new() -> Self {
        let ascii = use_ascii_glyphs();
        if no_color() {
            return Self {
                ascii,
                accent: Style::default().add_modifier(Modifier::BOLD),
                dim: Style::default().add_modifier(Modifier::DIM),
                green: Style::default().add_modifier(Modifier::BOLD),
                amber: Style::default().add_modifier(Modifier::BOLD),
            };
        }
        Self {
            ascii,
            accent: Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
            dim: Style::default().fg(Color::Gray).add_modifier(Modifier::DIM),
            green: Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
            amber: Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        }
    }
}

fn no_color() -> bool {
    std::env::var_os("NO_COLOR").is_some()
}

fn animation_disabled() -> bool {
    no_color() || std::env::var_os("EVERETT_NO_ANIM").is_some()
}

fn use_ascii_glyphs() -> bool {
    for var in ["LC_ALL", "LC_CTYPE", "LANG"] {
        if let Ok(v) = std::env::var(var) {
            return !v.to_lowercase().contains("utf");
        }
    }
    false
}

fn glyph(checked: bool, ascii: bool) -> &'static str {
    if ascii {
        if checked {
            "[x]"
        } else {
            "[ ]"
        }
    } else if checked {
        "☑"
    } else {
        "☐"
    }
}

fn stepper_text(value: i64, unit: &str) -> String {
    let plural = if value == 1 { "" } else { "s" };
    format!("‹ {} {}{} ›", value, unit, plural)
}

fn put(f: &mut Frame, y: u16, x: u16, text: &str, style: Style) {
    let area = f.area();
    if y >= area.height || x >= area.width {
        return;
    }
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(text.to_string(), style))),
        Rect::new(x, y, area.width - x, 1),
    );
}

/// Title bar + step indicator + progress rail + screen title. Returns the first content row.
fn draw_frame(f: &mut Frame, p: &Palette, step: usize, title: &str) -> Option<u16> {
    let area = f.area();
    if area.height < MIN_ROWS || area.width < MIN_COLS {
        if area.height == 0 || area.width == 0 {
            return None;
        }
        let msg = format!(
            "terminal too small ({}x{}) -- resize to at least {}x{}",
            area.width, area.height, MIN_COLS, MIN_ROWS
        );
        put(
            f,
            1.min(area.height - 1),
            (area.width.saturating_sub(msg.len() as u16)) / 2,
            &msg,
            p.amber,
        );
        put(
            f,
            area.height - 1,
            (area.width.saturating_sub(6)) / 2,
            "q quit",
            Style::default(),
        );
        return None;
    }
    let bar_style = p.accent.add_modifier(Modifier::REVERSED);
    let bar = format!(
        " {:<width$}",
        "everett · setup",
        width = area.width as usize - 1
    );
    put(f, 0, 0, &bar, bar_style);
    let indicator = format!("step {} of {}", step, STEP_TITLES.len());
    put(
        f,
        0,
        area.width.saturating_sub(indicator.len() as u16 + 2),
        &indicator,
        bar_style,
    );
    let rail = "●".repeat(step) + &"○".repeat(STEP_TITLES.len() - step);
    put(f, 1, 2, &rail, p.accent);
    put(
        f,
        2,
        2,
        title,
        Style::default().add_modifier(Modifier::BOLD),
    );
    Some(4)
}

fn footer(f: &mut Frame, p: &Palette, hint: &str) {
    let area = f.area();
    put(f, area.height - 1, 2, hint, p.dim);
}

fn read_key() -> TResult<KeyCode> {
    loop {
        match event::read() {
            Ok(Event::Key(k)) if k.kind == KeyEventKind::Press => return Ok(k.code),
            Ok(Event::Resize(_, _)) => return Ok(KeyCode::Null),
            Ok(_) => {}
            Err(_) => return Err(Quit),
        }
    }
}

fn is_quit(k: &KeyCode) -> bool {
    matches!(k, KeyCode::Char('q') | KeyCode::Esc)
}

fn pause(
    t: &mut Terminal<CrosstermBackend<io::Stdout>>,
    p: &Palette,
    step: usize,
    title: &str,
    lines: &[String],
    allow_back: bool,
    hint: Option<&str>,
) -> TResult<Dir> {
    let hint = hint.map(str::to_string).unwrap_or_else(|| {
        if allow_back {
            "enter continue   b back   q quit".into()
        } else {
            "enter continue   q quit".into()
        }
    });
    loop {
        t.draw(|f| {
            if let Some(mut y) = draw_frame(f, p, step, title) {
                for line in lines {
                    let style = if line.starts_with("  ") {
                        p.dim
                    } else {
                        Style::default()
                    };
                    put(f, y, 2, line, style);
                    y += 1;
                }
                footer(f, p, &hint);
            }
        })
        .map_err(|_| Quit)?;
        match read_key()? {
            k if is_quit(&k) => return Err(Quit),
            KeyCode::Enter | KeyCode::Char('a') => return Ok(Dir::Forward),
            KeyCode::Char('b') if allow_back => return Ok(Dir::Back),
            _ => {}
        }
    }
}

fn screen_welcome(t: &mut Terminal<CrosstermBackend<io::Stdout>>, p: &Palette) -> TResult<Dir> {
    let lines: Vec<String> = onboard::WELCOME_LINES
        .iter()
        .map(|s| s.to_string())
        .collect();
    pause(t, p, 1, "Welcome to Everett", &lines, false, None)
}

fn screen_detect(
    t: &mut Terminal<CrosstermBackend<io::Stdout>>,
    p: &Palette,
    cfg: &OnboardConfig,
) -> TResult<Dir> {
    let rows: Vec<String> = if cfg.detected.is_empty() {
        vec!["No coding-agent sessions or stores were found on this machine.".to_string()]
    } else {
        cfg.detected
            .iter()
            .map(|(h, c)| format!("{}  --  {} session(s) found", h, c))
            .collect()
    };
    if !animation_disabled() && !cfg.detected.is_empty() {
        let per = Duration::from_secs_f64(0.4 / rows.len() as f64);
        for n in 1..=rows.len() {
            t.draw(|f| {
                if let Some(y0) = draw_frame(f, p, 2, "What Everett found") {
                    for (i, row) in rows.iter().take(n).enumerate() {
                        put(
                            f,
                            y0 + i as u16,
                            2,
                            &format!("{} {}", glyph(true, p.ascii), row),
                            p.green,
                        );
                    }
                }
            })
            .map_err(|_| Quit)?;
            std::thread::sleep(per);
        }
    }
    let lines: Vec<String> = if cfg.detected.is_empty() {
        rows
    } else {
        rows.iter()
            .map(|r| format!("{} {}", glyph(true, p.ascii), r))
            .collect()
    };
    pause(t, p, 2, "What Everett found", &lines, true, None)
}

struct CheckItem {
    label: String,
    path: String,
    get: Box<dyn Fn(&OnboardConfig) -> bool>,
    set: Box<dyn Fn(&mut OnboardConfig, bool)>,
}

fn checklist(
    t: &mut Terminal<CrosstermBackend<io::Stdout>>,
    p: &Palette,
    step: usize,
    title: &str,
    intro: &[&str],
    items: &[CheckItem],
    cfg: &mut OnboardConfig,
) -> TResult<Dir> {
    let mut idx = 0usize;
    loop {
        t.draw(|f| {
            if let Some(mut y) = draw_frame(f, p, step, title) {
                for line in intro {
                    put(f, y, 2, line, p.dim);
                    y += 1;
                }
                y += 1;
                for (n, item) in items.iter().enumerate() {
                    let focused = n == idx;
                    let prefix = if focused { "›" } else { " " };
                    let style = if focused {
                        Style::default().add_modifier(Modifier::REVERSED)
                    } else {
                        Style::default()
                    };
                    put(
                        f,
                        y,
                        2,
                        &format!(
                            "{} {} {}",
                            prefix,
                            glyph((item.get)(cfg), p.ascii),
                            item.label
                        ),
                        style,
                    );
                    if !item.path.is_empty() {
                        put(f, y + 1, 6, &item.path, p.dim);
                    }
                    y += 2;
                }
                footer(
                    f,
                    p,
                    "up/down move   space toggle   enter continue   b back   q quit",
                );
            }
        })
        .map_err(|_| Quit)?;
        match read_key()? {
            k if is_quit(&k) => return Err(Quit),
            KeyCode::Up | KeyCode::Char('k') if !items.is_empty() => {
                idx = (idx + items.len() - 1) % items.len()
            }
            KeyCode::Down | KeyCode::Char('j') if !items.is_empty() => {
                idx = (idx + 1) % items.len()
            }
            KeyCode::Char(' ') if !items.is_empty() => {
                let cur = (items[idx].get)(cfg);
                (items[idx].set)(cfg, !cur);
            }
            KeyCode::Enter => return Ok(Dir::Forward),
            KeyCode::Char('b') => return Ok(Dir::Back),
            _ => {}
        }
    }
}

fn screen_hooks(
    t: &mut Terminal<CrosstermBackend<io::Stdout>>,
    p: &Palette,
    cfg: &mut OnboardConfig,
) -> TResult<Dir> {
    let mut items = Vec::new();
    for (harness, events) in &cfg.hooks {
        let path = if harness == "omp" {
            crate::install::omp_extension_path()
        } else {
            crate::install::settings_path(harness)
        };
        for event in events.keys() {
            let (h_get, h_set) = (harness.clone(), harness.clone());
            let (e_get, e_set) = (event.clone(), event.clone());
            items.push(CheckItem {
                label: format!("{}: {}", harness, crate::onboard::event_label(event)),
                path: path.display().to_string(),
                get: Box::new(move |c: &OnboardConfig| {
                    c.hooks
                        .iter()
                        .find(|(n, _)| *n == h_get)
                        .and_then(|(_, m)| m.get(&e_get))
                        .copied()
                        .unwrap_or(false)
                }),
                set: Box::new(move |c: &mut OnboardConfig, v| {
                    if let Some((_, m)) = c.hooks.iter_mut().find(|(n, _)| *n == h_set) {
                        m.insert(e_set.clone(), v);
                    }
                }),
            });
        }
    }
    checklist(
        t,
        p,
        3,
        "Hooks",
        &["Toggle which hooks to install. A backup is made of any file before it changes."],
        &items,
        cfg,
    )
}

fn screen_mcp(
    t: &mut Terminal<CrosstermBackend<io::Stdout>>,
    p: &Palette,
    cfg: &mut OnboardConfig,
) -> TResult<Dir> {
    let mut items = Vec::new();
    for (harness, _) in &cfg.mcp {
        let (h_get, h_set) = (harness.clone(), harness.clone());
        items.push(CheckItem {
            label: format!("{}: register the Everett MCP server", harness),
            path: crate::install::mcp_path(harness).display().to_string(),
            get: Box::new(move |c: &OnboardConfig| {
                c.mcp
                    .iter()
                    .find(|(n, _)| *n == h_get)
                    .map(|(_, v)| *v)
                    .unwrap_or(false)
            }),
            set: Box::new(move |c: &mut OnboardConfig, v| {
                if let Some((_, m)) = c.mcp.iter_mut().find(|(n, _)| *n == h_set) {
                    *m = v;
                }
            }),
        });
    }
    checklist(
        t,
        p,
        4,
        "MCP server",
        &["Lets each harness call Everett (ls / route / send / learn / card) as a native tool."],
        &items,
        cfg,
    )
}

/// Masked line editor. Empty enter = skip; q/Esc quits the whole wizard.
fn text_entry(
    t: &mut Terminal<CrosstermBackend<io::Stdout>>,
    p: &Palette,
    step: usize,
    title: &str,
    intro: &[&str],
) -> TResult<String> {
    let mut buf = String::new();
    loop {
        t.draw(|f| {
            if let Some(mut y) = draw_frame(f, p, step, title) {
                for line in intro {
                    put(f, y, 2, line, p.dim);
                    y += 1;
                }
                y += 1;
                put(
                    f,
                    y,
                    2,
                    &format!("> {}", "*".repeat(buf.chars().count())),
                    Style::default().add_modifier(Modifier::BOLD),
                );
                footer(
                    f,
                    p,
                    "type the key   enter to submit (empty = skip)   q quit",
                );
            }
        })
        .map_err(|_| Quit)?;
        match read_key()? {
            k if is_quit(&k) => return Err(Quit),
            KeyCode::Enter => return Ok(buf),
            KeyCode::Backspace => {
                buf.pop();
            }
            KeyCode::Char(c) => buf.push(c),
            _ => {}
        }
    }
}

fn screen_jev(
    t: &mut Terminal<CrosstermBackend<io::Stdout>>,
    p: &Palette,
    cfg: &mut OnboardConfig,
) -> TResult<Dir> {
    let (found_key, found_source) = onboard::find_jev_key_with_source();
    if !found_key.is_empty() && cfg.jev_found_source.is_empty() {
        cfg.jev_found_source = found_source;
    }
    loop {
        t.draw(|f| {
            if let Some(mut y) = draw_frame(f, p, 5, "Smarter routing (optional)") {
                for line in crate::onboard::JEV_LINES.iter() {
                    put(f, y, 2, line, p.dim);
                    y += 1;
                }
                y += 1;
                if !cfg.jev_found_source.is_empty() {
                    put(f, y, 2, &format!("Jev key found ✓ ({})", cfg.jev_found_source), p.green);
                    y += 2;
                }
                let note = if cfg.jev_choice == "paste" && !cfg.jev_key.is_empty() { " (key entered)" } else { "" };
                let paste_label = if cfg.jev_found_source.is_empty() { "paste a key" } else { "paste a different key" };
                put(f, y, 2, &format!("{} {}{}", glyph(cfg.jev_choice == "paste", p.ascii), paste_label, note), Style::default());
                y += 1;
                put(f, y, 2, &format!("{} use local matching", glyph(cfg.jev_choice == "skip", p.ascii)), Style::default());
                y += 2;
                if cfg!(target_os = "macos") {
                    put(f, y, 2, &format!("{} merge shared memory nightly (launchd, 04:00, --llm claude)", glyph(cfg.trunk_schedule_enabled, p.ascii)), Style::default());
                } else {
                    put(f, y, 2, "Automatic memory scheduling requires macOS.", Style::default());
                }
                footer(f, p, "p paste key   s skip   m toggle nightly merge   enter continue   b back   q quit");
            }
        })
        .map_err(|_| Quit)?;
        match read_key()? {
            k if is_quit(&k) => return Err(Quit),
            KeyCode::Char('p') => {
                let entered = text_entry(
                    t,
                    p,
                    5,
                    "Smarter routing (optional)",
                    &["Paste the Jev key (typesafe.ai)."],
                )?;
                if entered.is_empty() {
                    if cfg.jev_choice != "paste" {
                        cfg.jev_choice = "skip".to_string();
                    }
                    continue;
                }
                t.draw(|f| {
                    if let Some(y) = draw_frame(f, p, 5, "Smarter routing (optional)") {
                        put(f, y, 2, "validating…", p.dim);
                    }
                })
                .map_err(|_| Quit)?;
                if crate::route::verify_key(&entered) {
                    cfg.jev_key = entered;
                    cfg.jev_choice = "paste".to_string();
                    cfg.jev_validated = Some(true);
                    pause(
                        t,
                        p,
                        5,
                        "Smarter routing (optional)",
                        &["ok ✓ -- the key works.".to_string()],
                        false,
                        Some("enter continue"),
                    )?;
                } else {
                    let keep = loop {
                        t.draw(|f| {
                            if let Some(y) = draw_frame(f, p, 5, "Smarter routing (optional)") {
                                put(
                                    f,
                                    y,
                                    2,
                                    "failed -- Everett could not confirm this key.",
                                    p.amber,
                                );
                                footer(f, p, "k keep it anyway   s or enter to skip   q quit");
                            }
                        })
                        .map_err(|_| Quit)?;
                        match read_key()? {
                            k if is_quit(&k) => return Err(Quit),
                            KeyCode::Char('k') => break true,
                            KeyCode::Char('s') | KeyCode::Enter => break false,
                            _ => {}
                        }
                    };
                    if keep {
                        cfg.jev_key = entered;
                        cfg.jev_choice = "paste".to_string();
                        cfg.jev_validated = Some(false);
                    } else {
                        cfg.jev_choice = "skip".to_string();
                    }
                }
            }
            KeyCode::Char('s') => cfg.jev_choice = "skip".to_string(),
            KeyCode::Char('m') if cfg!(target_os = "macos") => {
                cfg.trunk_schedule_enabled = !cfg.trunk_schedule_enabled
            }
            KeyCode::Enter => return Ok(Dir::Forward),
            KeyCode::Char('b') => return Ok(Dir::Back),
            _ => {}
        }
    }
}

fn screen_backfill(
    t: &mut Terminal<CrosstermBackend<io::Stdout>>,
    p: &Palette,
    cfg: &mut OnboardConfig,
) -> TResult<Dir> {
    loop {
        let preview_len = if cfg.backfill_enabled {
            onboard::missing_card_sessions(cfg.span_days).len()
        } else {
            0
        };
        t.draw(|f| {
            if let Some(mut y) = draw_frame(f, p, 6, "Backfill cards (optional)") {
                put(
                    f,
                    y,
                    2,
                    "Make cards for your recent sessions. This step is skippable.",
                    p.dim,
                );
                y += 2;
                put(
                    f,
                    y,
                    2,
                    &format!("{} enabled", glyph(cfg.backfill_enabled, p.ascii)),
                    Style::default().add_modifier(Modifier::BOLD),
                );
                put(f, y, 20, "(space to toggle)", p.dim);
                y += 2;
                put(f, y, 2, "span:", Style::default());
                put(f, y, 8, &stepper_text(cfg.span_days, "day"), p.accent);
                put(f, y, 24, "(left/right or h/l, 1-30)", p.dim);
                y += 2;
                if cfg.backfill_enabled {
                    put(
                        f,
                        y,
                        2,
                        &format!("{} session(s) without a card in that span", preview_len),
                        p.green,
                    );
                    y += 2;
                }
                put(
                    f,
                    y,
                    2,
                    "no LLM calls; cards are deterministic and marked <!-- everett:auto -->",
                    p.dim,
                );
                footer(
                    f,
                    p,
                    "space toggle   ‹/› or h/l span   enter continue   b back   q quit",
                );
            }
        })
        .map_err(|_| Quit)?;
        match read_key()? {
            k if is_quit(&k) => return Err(Quit),
            KeyCode::Char(' ') => cfg.backfill_enabled = !cfg.backfill_enabled,
            KeyCode::Left | KeyCode::Char('h') => cfg.span_days = (cfg.span_days - 1).max(1),
            KeyCode::Right | KeyCode::Char('l') => cfg.span_days = (cfg.span_days + 1).min(30),
            KeyCode::Enter => return Ok(Dir::Forward),
            KeyCode::Char('b') => return Ok(Dir::Back),
            _ => {}
        }
    }
}

fn confirm_lines(cfg: &OnboardConfig) -> Vec<String> {
    let mut lines = vec![
        "Nothing is written until you apply.".to_string(),
        String::new(),
        "Hooks:".to_string(),
    ];
    for (harness, path, selected) in hook_change_lines(cfg) {
        lines.push(format!(
            "  {:<7} {:<7} {}",
            if selected { "install" } else { "skip" },
            harness,
            path
        ));
    }
    lines.push("MCP:".to_string());
    for (harness, on) in &cfg.mcp {
        lines.push(format!(
            "  {:<8} {:<7} {}",
            if *on { "register" } else { "skip" },
            harness,
            crate::install::mcp_path(harness).display()
        ));
    }
    lines.push("Smarter routing (Jev):".to_string());
    if cfg.jev_choice == "paste" && !cfg.jev_key.is_empty() {
        lines.push(format!(
            "  save key ({})",
            if cfg.jev_validated.unwrap_or(false) {
                "validated"
            } else {
                "validation failed, kept anyway"
            }
        ));
    } else if !cfg.jev_found_source.is_empty() {
        lines.push(format!(
            "  skip (using found key: {})",
            cfg.jev_found_source
        ));
    } else {
        lines.push("  skip (local matching)".to_string());
    }
    lines.push("Nightly merge:".to_string());
    lines.push(if cfg.trunk_schedule_enabled {
        "  schedule `everett trunk merge` at 04:00 via launchd".to_string()
    } else {
        "  skip".to_string()
    });
    lines.push("Backfill:".to_string());
    lines.push(if cfg.backfill_enabled {
        format!("  generate cards, last {} day(s)", cfg.span_days)
    } else {
        "  skip".to_string()
    });
    lines
}

/// (harness, settings-path, any-selected) for the confirm screen.
fn hook_change_lines(cfg: &OnboardConfig) -> Vec<(String, String, bool)> {
    cfg.hooks
        .iter()
        .map(|(harness, events)| {
            let path = if harness == "omp" {
                crate::install::omp_extension_path()
            } else {
                crate::install::settings_path(harness)
            };
            (
                harness.clone(),
                path.display().to_string(),
                events.values().any(|v| *v),
            )
        })
        .collect()
}

fn screen_confirm(
    t: &mut Terminal<CrosstermBackend<io::Stdout>>,
    p: &Palette,
    cfg: &OnboardConfig,
) -> TResult<Dir> {
    match pause(
        t,
        p,
        STEP_TITLES.len(),
        "Apply these changes?",
        &confirm_lines(cfg),
        true,
        Some("enter/a apply   b back   q quit"),
    )? {
        Dir::Forward => Ok(Dir::Apply),
        other => Ok(other),
    }
}

fn draw_apply(f: &mut Frame, p: &Palette, steps: &[&str], done: &[&str], extra: Option<&str>) {
    put(
        f,
        0,
        2,
        "Applying...",
        Style::default().add_modifier(Modifier::BOLD),
    );
    let mut y = 2;
    for name in steps {
        let (mark, style) = if done.contains(name) {
            (glyph(true, p.ascii), p.green)
        } else {
            (glyph(false, p.ascii), p.dim)
        };
        put(f, y, 2, &format!("{} {}", mark, name), style);
        y += 1;
    }
    if let Some(line) = extra {
        put(f, y + 1, 2, line, p.accent);
    }
}

fn apply_draw(
    t: &mut Terminal<CrosstermBackend<io::Stdout>>,
    p: &Palette,
    steps: &[&str],
    done: &[&str],
    extra: Option<&str>,
) -> TResult<()> {
    t.draw(|f| draw_apply(f, p, steps, done, extra))
        .map(|_| ())
        .map_err(|_| Quit)
}

fn screen_apply(
    t: &mut Terminal<CrosstermBackend<io::Stdout>>,
    p: &Palette,
    cfg: &OnboardConfig,
) -> TResult<onboard::ApplyResult> {
    let mut steps = vec!["Hooks", "MCP", "Jev", "Merge schedule"];
    if cfg.backfill_enabled {
        steps.push("Backfill");
    }
    let mut done: Vec<&str> = Vec::new();

    apply_draw(t, p, &steps, &done, None)?;
    let hook_lines = onboard::apply_hooks(cfg);
    done.push("Hooks");
    apply_draw(t, p, &steps, &done, None)?;
    let mcp_lines = onboard::apply_mcp(cfg);
    done.push("MCP");
    apply_draw(t, p, &steps, &done, None)?;
    let jev_line = onboard::apply_jev(cfg);
    done.push("Jev");
    apply_draw(t, p, &steps, &done, None)?;
    let trunk_schedule_line = onboard::apply_trunk_schedule(cfg);
    done.push("Merge schedule");
    apply_draw(t, p, &steps, &done, None)?;

    let (mut written, mut skipped) = (0usize, 0usize);
    if cfg.backfill_enabled {
        // Live progress bar while each card is generated; the terminal goes through a RefCell
        // because the progress callback is `&dyn Fn`, not `FnMut`.
        let term_cell = std::cell::RefCell::new(&mut *t);
        let steps_ref = &steps;
        let done_ref = &done;
        let progress = |i: usize, total: usize| {
            let bar_w = 30usize;
            let filled = if total > 0 { bar_w * i / total } else { bar_w };
            let bar = "#".repeat(filled) + &"-".repeat(bar_w - filled);
            let extra = format!("[{}] {}/{}", bar, i, total);
            let _ = term_cell
                .borrow_mut()
                .draw(|f| draw_apply(f, p, steps_ref, done_ref, Some(&extra)));
        };
        (written, skipped) = onboard::generate_backfill_cards(
            &onboard::missing_card_sessions(cfg.span_days),
            Some(&progress),
        );
        done.push("Backfill");
        apply_draw(t, p, &steps, &done, None)?;
    }
    Ok(onboard::ApplyResult {
        hook_lines,
        mcp_lines,
        backfill_written: written,
        backfill_skipped: skipped,
        jev_line,
        trunk_schedule_line,
    })
}

fn try_this_box(f: &mut Frame, p: &Palette, y: u16) {
    let w = f.area().width;
    let commands = [
        "everett ls",
        "everett route \"what is my codex session doing?\"",
    ];
    let prompt = "use everett to ask my <name> session what it is doing";
    let inner_w = (w.saturating_sub(6) as usize).min(
        commands
            .iter()
            .map(|c| c.len())
            .chain([prompt.len()])
            .max()
            .unwrap_or(0)
            + 4,
    );
    let box_w = inner_w + 4;
    let x = 2u16;
    put(
        f,
        y,
        x,
        &format!("┌{}┐", "─".repeat(box_w.saturating_sub(2))),
        p.accent,
    );
    put(
        f,
        y + 1,
        x,
        &format!("│ Try this{}│", " ".repeat(box_w.saturating_sub(11))),
        p.accent,
    );
    let mut row = y + 2;
    for cmd in commands {
        put(f, row, x, "│", p.accent);
        put(f, row, x + 2, &format!("$ {}", cmd), p.green);
        put(f, row, x + box_w as u16 - 1, "│", p.accent);
        row += 1;
    }
    put(f, row, x, "│", p.accent);
    put(f, row, x + 2, "sample agent prompt:", p.dim);
    put(f, row, x + box_w as u16 - 1, "│", p.accent);
    row += 1;
    put(f, row, x, "│", p.accent);
    put(f, row, x + 2, &format!("\"{}\"", prompt), Style::default());
    put(f, row, x + box_w as u16 - 1, "│", p.accent);
    row += 1;
    put(
        f,
        row,
        x,
        &format!("└{}┘", "─".repeat(box_w.saturating_sub(2))),
        p.accent,
    );
}

fn screen_summary(
    t: &mut Terminal<CrosstermBackend<io::Stdout>>,
    p: &Palette,
    cfg: &OnboardConfig,
    result: &onboard::ApplyResult,
) -> TResult<()> {
    let lines = onboard::summary_lines(cfg, result);
    loop {
        t.draw(|f| {
            if let Some(mut y) = draw_frame(f, p, STEP_TITLES.len(), "Done") {
                for line in &lines {
                    let style = if line.starts_with("  ") {
                        p.dim
                    } else {
                        Style::default()
                    };
                    put(f, y, 2, line, style);
                    y += 1;
                }
                try_this_box(f, p, y + 1);
                footer(f, p, "enter/q to exit");
            }
        })
        .map_err(|_| Quit)?;
        match read_key()? {
            k if is_quit(&k) => return Ok(()),
            KeyCode::Enter | KeyCode::Char('a') => return Ok(()),
            _ => {}
        }
    }
}

struct TermGuard;

impl Drop for TermGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, cursor::Show);
    }
}

/// The wizard. `Err(())` means the terminal could not be initialized — caller falls back to
/// plain prompts. `Err(Quit)` semantics travel via `Ok(None)`.
pub fn run(args: &crate::cli::Args, mut cfg: OnboardConfig) -> Result<Option<()>, ()> {
    use std::io::IsTerminal;
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(());
    }
    match std::env::var("TERM") {
        Ok(t) if t != "dumb" => {}
        _ => return Err(()),
    }
    enable_raw_mode().map_err(|_| ())?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, cursor::Hide).map_err(|_| ())?;
    let backend = CrosstermBackend::new(stdout);
    let mut term = Terminal::new(backend).map_err(|_| ())?;
    term.clear().map_err(|_| ())?;
    let _guard = TermGuard;
    let p = Palette::new();

    cfg.span_days = args.span_days;
    if args.no_backfill {
        cfg.backfill_enabled = false;
    }
    if args.no_mcp {
        cfg.mcp = cfg.mcp.iter().map(|(h, _)| (h.clone(), false)).collect();
    }

    let mut i = 0usize;
    while i < STEP_TITLES.len() {
        let dir = match i {
            0 => screen_welcome(&mut term, &p),
            1 => screen_detect(&mut term, &p, &cfg),
            2 => screen_hooks(&mut term, &p, &mut cfg),
            3 => screen_mcp(&mut term, &p, &mut cfg),
            4 => screen_jev(&mut term, &p, &mut cfg),
            5 => screen_backfill(&mut term, &p, &mut cfg),
            _ => screen_confirm(&mut term, &p, &cfg),
        };
        match dir {
            Err(Quit) => return Ok(None),
            Ok(Dir::Back) => i = i.saturating_sub(1),
            Ok(Dir::Apply) => match screen_apply(&mut term, &p, &cfg) {
                Err(Quit) => return Ok(None),
                Ok(result) => {
                    let _ = screen_summary(&mut term, &p, &cfg, &result);
                    return Ok(Some(()));
                }
            },
            Ok(Dir::Forward) => i += 1,
        }
    }
    Ok(Some(()))
}
