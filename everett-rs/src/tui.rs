//! Human onboarding. Account approval is separate from reviewed local configuration.

use std::io;
use std::time::{Duration, Instant};

use crossterm::cursor;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::{Frame, Terminal};

use crate::onboard::{self, OnboardConfig};

const STEP_TITLES: &[&str] = &[
    "Welcome",
    "Account",
    "Agents",
    "Setup",
    "Review",
    "Instructions",
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
    cyan: Style,
    violet: Style,
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
                cyan: Style::default().add_modifier(Modifier::BOLD),
                violet: Style::default().add_modifier(Modifier::BOLD),
            };
        }
        Self {
            ascii,
            accent: Style::default()
                .fg(Color::Indexed(209))
                .add_modifier(Modifier::BOLD),
            dim: Style::default().add_modifier(Modifier::DIM),
            green: Style::default()
                .fg(Color::Indexed(114))
                .add_modifier(Modifier::BOLD),
            amber: Style::default()
                .fg(Color::Indexed(221))
                .add_modifier(Modifier::BOLD),
            cyan: Style::default()
                .fg(Color::Indexed(81))
                .add_modifier(Modifier::BOLD),
            violet: Style::default()
                .fg(Color::Indexed(141))
                .add_modifier(Modifier::BOLD),
        }
    }
}

fn no_color() -> bool {
    std::env::var_os("NO_COLOR").is_some()
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
    put(f, 0, 2, "everett / setup", p.accent);
    let indicator = format!("{} / {}", step, STEP_TITLES.len());
    put(
        f,
        0,
        area.width.saturating_sub(indicator.len() as u16 + 2),
        &indicator,
        p.dim,
    );
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

fn primary_action(f: &mut Frame, p: &Palette, area: Rect, label: &str) {
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                if p.ascii { " Enter " } else { " Enter ↵ " },
                Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED),
            ),
            Span::raw("  "),
            Span::styled(label, Style::default().add_modifier(Modifier::BOLD)),
            Span::styled(if p.ascii { " ->" } else { " →" }, p.accent),
        ])),
        Rect::new(area.x, area.y + 1, area.width, 1),
    );
}

struct Motion {
    started: Instant,
    enabled: bool,
}

impl Motion {
    fn new() -> Self {
        Self {
            started: Instant::now(),
            enabled: std::env::var_os("EVERETT_NO_ANIM").is_none(),
        }
    }

    fn elapsed(&self) -> f64 {
        if self.enabled {
            self.started.elapsed().as_secs_f64()
        } else {
            6.0
        }
    }

    fn restart(&mut self) {
        self.started = Instant::now();
    }

    fn read(&self, seconds: f64) -> TResult<KeyCode> {
        read_key_for(if self.elapsed() < seconds {
            Some(Duration::from_millis(50))
        } else {
            None
        })
    }
}

fn read_key() -> TResult<KeyCode> {
    read_key_for(None)
}

fn read_key_for(timeout: Option<Duration>) -> TResult<KeyCode> {
    loop {
        if let Some(timeout) = timeout {
            if !event::poll(timeout).map_err(|_| Quit)? {
                return Ok(KeyCode::Null);
            }
        }
        match event::read() {
            Ok(Event::Key(k)) if k.kind == KeyEventKind::Press => {
                let code = if k.code == KeyCode::Char('c')
                    && k.modifiers.contains(KeyModifiers::CONTROL)
                {
                    KeyCode::Esc
                } else {
                    k.code
                };
                let (cols, rows) = crossterm::terminal::size().map_err(|_| Quit)?;
                if !is_quit(&code) && (cols < MIN_COLS || rows < MIN_ROWS) {
                    return Ok(KeyCode::Null);
                }
                return Ok(code);
            }
            Ok(Event::Resize(_, _)) => return Ok(KeyCode::Null),
            Ok(_) => {}
            Err(_) => return Err(Quit),
        }
    }
}

fn draw_balloon(f: &mut Frame, p: &Palette, area: Rect, time: f64) {
    if p.ascii {
        let x = area.x + area.width / 2 - 3;
        let y = area.y + area.height / 2 - 2;
        for (row, line) in [" .---. ", "(     )", " \\___/ ", "   v   ", "   )   "]
            .iter()
            .enumerate()
        {
            put(
                f,
                y + row as u16,
                x,
                line,
                if row < 4 { p.accent } else { p.cyan },
            );
        }
        return;
    }
    let width = area.width as usize;
    let height = area.height as usize;
    let mut dots = vec![0u8; width * height];
    let mut hues = vec![0usize; width * height];
    let scale = (width as f64 * 2.0 / 58.0).min(height as f64 * 4.0 / 44.0);
    let time = time.min(6.0);
    let bob = (time * 2.4).sin() * 1.4;
    let sway = (time * 1.6).sin() * 1.2;
    let cx = width as f64 + sway * scale;
    let cy = (16.0 + bob) * scale;
    for py in 0..height * 4 {
        for px in 0..width * 2 {
            let x = (px as f64 - cx) / scale;
            let y = (py as f64 - cy) / scale;
            let ny = y / 14.0;
            let nx = x / (15.0 * (1.0 - ny * 0.18));
            let body = nx * nx + ny * ny <= 1.0;
            let knot = (14.0..=17.0).contains(&y) && x.abs() <= (y - 13.0) * 0.7;
            let string_y = y - 17.0;
            let string_x = (string_y * 0.38 + time * 1.8).sin() * (string_y * 0.14);
            let string = (0.0..=10.0).contains(&string_y) && (x - string_x).abs() < 0.65;
            if !body && !knot && !string {
                continue;
            }
            let index = (py / 4) * width + px / 2;
            let bit = [[0, 1, 2, 6], [3, 4, 5, 7]][px % 2][py % 4];
            dots[index] |= 1 << bit;
            let highlight =
                ((x + 5.5 + (time * 1.3).sin()) / 3.0).powi(2) + ((y + 5.0) / 6.0).powi(2) < 1.0;
            hues[index] = if string && !body && !knot {
                3
            } else if highlight {
                1
            } else if nx > 0.45 || ny > 0.72 {
                2
            } else {
                0
            };
        }
    }
    for y in 0..height {
        for x in 0..width {
            let index = y * width + x;
            if dots[index] == 0 {
                continue;
            }
            let style = [p.accent, p.amber, p.violet, p.cyan][hues[index]];
            let glyph = char::from_u32(0x2800 + dots[index] as u32).unwrap();
            put(
                f,
                area.y + y as u16,
                area.x + x as u16,
                &glyph.to_string(),
                style,
            );
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
    let mut scroll = 0u16;
    let mut max_scroll = 0u16;
    loop {
        t.draw(|f| {
            if let Some(y) = draw_frame(f, p, step, title) {
                let area = f.area();
                let mut wrapped = Vec::new();
                for line in lines {
                    let mut row = String::new();
                    for character in line.chars() {
                        let mut next = row.clone();
                        next.push(character);
                        if Line::from(next.as_str()).width() > (area.width - 4) as usize {
                            wrapped.push(Line::from(std::mem::take(&mut row)));
                        }
                        row.push(character);
                    }
                    wrapped.push(Line::from(row));
                }
                let height = area.height - y - 2;
                max_scroll = wrapped
                    .len()
                    .saturating_sub(height as usize)
                    .min(u16::MAX as usize) as u16;
                let paragraph = Paragraph::new(wrapped);
                scroll = scroll.min(max_scroll);
                f.render_widget(
                    paragraph.scroll((scroll, 0)),
                    Rect::new(2, y, area.width - 4, height),
                );
                footer(f, p, &hint);
            }
        })
        .map_err(|_| Quit)?;
        match read_key()? {
            k if is_quit(&k) => return Err(Quit),
            KeyCode::Enter => return Ok(Dir::Forward),
            KeyCode::Char('b') if allow_back => return Ok(Dir::Back),
            KeyCode::Down | KeyCode::Char('j') => scroll = (scroll + 1).min(max_scroll),
            KeyCode::Up | KeyCode::Char('k') => scroll = scroll.saturating_sub(1),
            KeyCode::PageDown => scroll = scroll.saturating_add(5).min(max_scroll),
            KeyCode::PageUp => scroll = scroll.saturating_sub(5),
            KeyCode::Home => scroll = 0,
            KeyCode::End => scroll = max_scroll,
            _ => {}
        }
    }
}

fn screen_welcome(t: &mut Terminal<CrosstermBackend<io::Stdout>>, p: &Palette) -> TResult<Dir> {
    let motion = Motion::new();
    loop {
        t.draw(|f| {
            let Some(y) = draw_frame(f, p, 1, "Welcome to Everett") else {
                return;
            };
            let area = f.area();
            if area.width >= 76 && area.height >= 22 {
                let top = y + (area.height - 20) / 2;
                draw_balloon(f, p, Rect::new(3, top, 29, 11), motion.elapsed());
                put(f, top + 1, 36, "Your agents, connected.", p.cyan);
                put(
                    f,
                    top + 3,
                    36,
                    "The agents you already use.",
                    Style::default(),
                );
                put(f, top + 4, 36, "On this computer and beyond.", p.violet);
                put(
                    f,
                    top + 5,
                    36,
                    "You choose what to connect.",
                    Style::default(),
                );
                primary_action(
                    f,
                    p,
                    Rect::new(36, top + 7, area.width - 38, 3),
                    "Sign in and get started",
                );
                put(f, area.height - 4, 4, "Sign in. Choose. Connect.", p.dim);
            } else {
                put(f, y, 2, "Your agents, connected.", p.cyan);
                put(f, y + 2, 2, "Here and beyond this computer.", p.violet);
                put(f, y + 4, 2, "Review every change first.", p.dim);
                if area.width >= 54 {
                    draw_balloon(f, p, Rect::new(area.width - 18, y, 16, 6), motion.elapsed());
                }
                primary_action(
                    f,
                    p,
                    Rect::new(2, area.height - 4, area.width - 4, 3),
                    "Sign in and get started",
                );
            }
            footer(f, p, "d explore Everett   q quit");
        })
        .map_err(|_| Quit)?;
        match motion.read(6.0)? {
            k if is_quit(&k) => return Err(Quit),
            KeyCode::Enter => return Ok(Dir::Forward),
            KeyCode::Char('d') => {
                screen_explanation(t, p)?;
            }
            _ => {}
        }
    }
}

fn screen_account(t: &mut Terminal<CrosstermBackend<io::Stdout>>, p: &Palette) -> TResult<Dir> {
    let configured = crate::cloudflare_auth::available();
    let mut attempt = configured;
    let mut verified = false;
    let mut status = if configured {
        "Opening browser sign-in…".to_string()
    } else {
        "Account sign-in is not available in this build yet.".to_string()
    };
    loop {
        t.draw(|f| {
            if let Some(y) = draw_frame(f, p, 2, "Sign in to Everett") {
                let area = f.area();
                f.render_widget(
                    Paragraph::new(status.as_str())
                        .style(if verified { p.green } else { p.amber })
                        .wrap(Wrap { trim: false }),
                    Rect::new(2, y, area.width - 4, area.height - y - 6),
                );
                primary_action(
                    f,
                    p,
                    Rect::new(2, area.height - 4, area.width - 4, 3),
                    if verified {
                        "Choose your agents"
                    } else if configured {
                        "Retry browser sign-in"
                    } else {
                        "Continue on this computer"
                    },
                );
                footer(
                    f,
                    p,
                    if verified {
                        "enter continue   b back   q quit"
                    } else {
                        "l continue locally   b back   q quit"
                    },
                );
            }
        })
        .map_err(|_| Quit)?;
        if attempt {
            attempt = false;
            disable_raw_mode().map_err(|_| Quit)?;
            execute!(t.backend_mut(), LeaveAlternateScreen, cursor::Show).map_err(|_| Quit)?;
            let result = crate::cloudflare_auth::sign_in(&crate::cli::Args {
                open: std::env::var_os("SSH_CONNECTION").is_none()
                    && std::env::var_os("SSH_TTY").is_none(),
                auth_provider: Some("cloudflare".into()),
                ..Default::default()
            });
            enable_raw_mode().map_err(|_| Quit)?;
            execute!(t.backend_mut(), EnterAlternateScreen, cursor::Hide).map_err(|_| Quit)?;
            t.clear().map_err(|_| Quit)?;
            status = match result {
                Ok(identity) => {
                    verified = identity["verified"] == true;
                    format!("Signed in. Identity verified for {}.\nBrowser approval complete; you can close that page.", identity["device_name"].as_str().unwrap_or("this computer"))
                }
                Err(error) => format!("Sign-in did not complete: {error}"),
            };
            continue;
        }
        match read_key()? {
            k if is_quit(&k) => return Err(Quit),
            KeyCode::Char('b') => return Ok(Dir::Back),
            KeyCode::Char('l') if !verified => return Ok(Dir::Forward),
            KeyCode::Enter if verified || !configured => return Ok(Dir::Forward),
            KeyCode::Enter => {
                attempt = true;
            }
            _ => {}
        }
    }
}

#[derive(Clone, Copy)]
enum ExplanationTopic {
    Overview,
    Local,
    Cloud,
    Vm,
}

const EXPLANATION_TOPICS: &[(&str, ExplanationTopic)] = &[
    ("Overview", ExplanationTopic::Overview),
    ("Local", ExplanationTopic::Local),
    ("Cloud", ExplanationTopic::Cloud),
    ("VM", ExplanationTopic::Vm),
];

fn explanation_lines(topic: ExplanationTopic, p: &Palette) -> Vec<Line<'static>> {
    let lines = match topic {
        ExplanationTopic::Overview => vec![
            ("Hand off work", p.cyan),
            (
                "A cloud assistant asks a coding agent for help.",
                Style::default(),
            ),
            ("", p.dim),
            ("Share context", p.violet),
            (
                "Give selected agents the same shared notes.",
                Style::default(),
            ),
            ("", p.dim),
            ("Get a reply", p.accent),
            (
                "Bring results back to the agent that asked.",
                Style::default(),
            ),
            ("", p.dim),
            ("Dots / Grok Bot / local coding agents", p.dim),
            ("Muse and Hark: compatibility needs checking.", p.amber),
        ],
        ExplanationTopic::Local => vec![
            ("Agents on this computer", p.cyan),
            ("Claude Code / Codex / OMP / Pi / Hermes", Style::default()),
            ("Grok CLI / Devin CLI", Style::default()),
            ("", p.dim),
            (
                "Find sessions and pass work between them.",
                Style::default(),
            ),
            ("Use shared notes and exchange replies.", Style::default()),
            ("", p.dim),
            ("How they connect", p.violet),
            (
                "Everett reads existing local session stores.",
                Style::default(),
            ),
            ("Hooks and MCP setup vary by agent.", Style::default()),
            ("No Everett account is needed for local setup.", p.dim),
        ],
        ExplanationTopic::Cloud => vec![
            ("Cloud assistants", p.cyan),
            ("OpenAI Dots / Grok Bot", Style::default()),
            ("Hark: connection support needs verification.", p.amber),
            ("", p.dim),
            (
                "Give an assistant Everett tools to send work",
                Style::default(),
            ),
            ("and read replies from its own inbox.", Style::default()),
            ("", p.dim),
            ("How they connect", p.violet),
            ("Owner setup and approval are required.", Style::default()),
            (
                "The connection needs a reachable MCP gateway.",
                Style::default(),
            ),
            ("Queued work is not a confirmed reply.", p.dim),
        ],
        ExplanationTopic::Vm => vec![
            ("Agents with their own computer", p.cyan),
            ("Muse / another agent running in a VM", Style::default()),
            ("Muse integration has not been verified.", p.amber),
            ("", p.dim),
            (
                "A VM agent could join your other machines",
                Style::default(),
            ),
            ("to exchange work and results.", Style::default()),
            ("", p.dim),
            ("What needs checking", p.violet),
            (
                "Can it run Everett and keep its credentials?",
                Style::default(),
            ),
            ("Can it reach the service from its VM?", Style::default()),
            (
                "Does the connection survive a VM restart?",
                Style::default(),
            ),
        ],
    };
    lines
        .into_iter()
        .map(|(text, style)| Line::styled(text, style))
        .collect()
}

fn screen_explanation(t: &mut Terminal<CrosstermBackend<io::Stdout>>, p: &Palette) -> TResult<Dir> {
    let mut topic = 0usize;
    let mut scroll = 0usize;
    let mut page_height = 1usize;
    let mut max_scroll = 0usize;
    loop {
        t.draw(|f| {
            let Some(y) = draw_frame(f, p, 2, "What can Everett do?") else {
                return;
            };
            let area = f.area();
            let tabs_y = if area.height >= 20 {
                put(
                    f,
                    y,
                    2,
                    "Let your agents work together, wherever they run.",
                    p.dim,
                );
                y + 2
            } else {
                y
            };
            let tabs: Vec<_> = EXPLANATION_TOPICS
                .iter()
                .enumerate()
                .flat_map(|(i, (name, _))| {
                    vec![
                        Span::styled(
                            format!(" {} {} ", i + 1, name),
                            if i == topic {
                                p.cyan.add_modifier(Modifier::REVERSED)
                            } else {
                                p.dim
                            },
                        ),
                        Span::raw("  "),
                    ]
                })
                .collect();
            f.render_widget(
                Paragraph::new(Line::from(tabs)),
                Rect::new(2, tabs_y, area.width - 4, 1),
            );
            let content_y = tabs_y + 2;
            let lines = explanation_lines(EXPLANATION_TOPICS[topic].1, p);
            page_height = (area.height - content_y - 5) as usize;
            max_scroll = lines.len().saturating_sub(page_height);
            scroll = scroll.min(max_scroll);
            f.render_widget(
                Paragraph::new(lines).scroll((scroll as u16, 0)),
                Rect::new(2, content_y, area.width - 4, page_height as u16),
            );
            if max_scroll > 0 {
                put(
                    f,
                    area.height - 5,
                    2,
                    &format!(
                        "j/k scroll   {}-{} of {}",
                        scroll + 1,
                        scroll + page_height,
                        max_scroll + page_height
                    ),
                    p.dim,
                );
            }
            primary_action(
                f,
                p,
                Rect::new(2, area.height - 4, area.width - 4, 3),
                "Check this computer",
            );
            footer(f, p, "1-4 explore   j/k scroll   b back   q quit");
        })
        .map_err(|_| Quit)?;
        match read_key()? {
            key if is_quit(&key) => return Err(Quit),
            KeyCode::Enter => return Ok(Dir::Forward),
            KeyCode::Char('b') => return Ok(Dir::Back),
            KeyCode::Char(key @ '1'..='4') => {
                topic = key as usize - '1' as usize;
                scroll = 0;
            }
            KeyCode::Right | KeyCode::Tab => {
                topic = (topic + 1) % EXPLANATION_TOPICS.len();
                scroll = 0;
            }
            KeyCode::Left | KeyCode::BackTab => {
                topic = (topic + EXPLANATION_TOPICS.len() - 1) % EXPLANATION_TOPICS.len();
                scroll = 0;
            }
            KeyCode::Down | KeyCode::Char('j') => scroll = (scroll + 1).min(max_scroll),
            KeyCode::Up | KeyCode::Char('k') => scroll = scroll.saturating_sub(1),
            KeyCode::PageDown => scroll = (scroll + page_height).min(max_scroll),
            KeyCode::PageUp => scroll = scroll.saturating_sub(page_height),
            KeyCode::Home => scroll = 0,
            KeyCode::End => scroll = max_scroll,
            _ => {}
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RemoteConnection {
    Dots,
    GrokBot,
    Hark,
    Muse,
    Computer,
}

impl RemoteConnection {
    fn name(self) -> &'static str {
        match self {
            Self::Dots => "OpenAI Dots",
            Self::GrokBot => "Grok Bot",
            Self::Hark => "Hark",
            Self::Muse => "Muse",
            Self::Computer => "Another computer",
        }
    }

    fn status(self) -> &'static str {
        match self {
            Self::Dots | Self::GrokBot | Self::Computer => "setup needed",
            Self::Hark | Self::Muse => "unverified",
        }
    }

    fn guide(self) -> Vec<String> {
        let lines: &[&str] = match self {
            Self::Dots => &[
                "A Dots connection needs your approval.",
                "The current path uses Secure MCP Tunnel.",
                "On the computer with your agent sessions:",
                "1. Give this Dot its own gateway identity.",
                "2. Choose the sessions it may message.",
                "3. Approve the tunnel in your Dots workspace.",
                "4. Ask the Dot to read its Everett identity.",
                "https://github.com/raitoxlol/everett/blob/main/docs/openai-dots.md",
            ],
            Self::GrokBot => &[
                "A Grok Bot needs its own gateway identity.",
                "1. Choose the sessions it may message.",
                "2. Provide a reachable HTTPS MCP gateway.",
                "3. Approve the connector in Grok.",
                "4. Ask the Bot to read its Everett identity.",
                "https://github.com/raitoxlol/everett/blob/main/docs/grok-bot.md",
            ],
            Self::Computer => &[
                "Run Everett on that computer too.",
                "Pair it with this computer using everett peer.",
                "Check the encrypted link before sending work.",
                "Then test a send, inbox read and reply.",
                "Use everett peer --help for current commands.",
            ],
            Self::Muse => &[
                "Muse has not been tested with Everett.",
                "Check whether your Muse environment can:",
                "1. Run a persistent Everett process.",
                "2. Store its credentials privately.",
                "3. Reach your other computer or MCP gateway.",
                "4. Keep the connection after a VM restart.",
                "A VM alone does not establish compatibility.",
            ],
            Self::Hark => &[
                "Hark has not been tested with Everett.",
                "Check whether your Hark client can add an",
                "authenticated MCP connector or run Everett.",
                "Computer control alone does not prove this.",
                "Verify send, inbox read and reply before use.",
            ],
        };
        lines
            .iter()
            .map(|line| line.to_string())
            .chain([
                String::new(),
                "This wizard cannot finish that connection yet.".into(),
                "It stays pending; no client grant is added.".into(),
            ])
            .collect()
    }
}

fn connection_tabs(f: &mut Frame, p: &Palette, tab: usize) {
    let tabs = ["Local", "Cloud", "VM / computer"]
        .iter()
        .enumerate()
        .flat_map(|(i, name)| {
            [
                Span::styled(
                    format!(" {} {} ", i + 1, name),
                    if i == tab {
                        p.cyan.add_modifier(Modifier::REVERSED)
                    } else {
                        p.dim
                    },
                ),
                Span::raw(" "),
            ]
        })
        .collect::<Vec<_>>();
    f.render_widget(
        Paragraph::new(Line::from(tabs)),
        Rect::new(2, 4, f.area().width - 4, 1),
    );
}

struct AgentChoice {
    harness: String,
    sessions: usize,
    events: Vec<String>,
    mcp: bool,
}

impl AgentChoice {
    fn choices(cfg: &OnboardConfig, allow_mcp: bool) -> Vec<Self> {
        cfg.detected
            .iter()
            .map(|(harness, sessions)| {
                let mut events: Vec<_> = cfg
                    .hooks
                    .iter()
                    .find(|(h, _)| h == harness)
                    .map(|(_, events)| events.keys().cloned().collect())
                    .unwrap_or_default();
                events.sort();
                Self {
                    harness: harness.clone(),
                    sessions: *sessions,
                    events,
                    mcp: allow_mcp && cfg.mcp.iter().any(|(h, _)| h == harness),
                }
            })
            .collect()
    }

    fn name(&self) -> &str {
        match self.harness.as_str() {
            "claude" => "Claude Code",
            "codex" => "Codex",
            "omp" => "OMP",
            "grok" => "Grok CLI",
            "devin" => "Devin CLI",
            "pi" => "Pi",
            "hermes" => "Hermes",
            other => other,
        }
    }

    fn selected(&self, cfg: &OnboardConfig) -> usize {
        let hooks = cfg
            .hooks
            .iter()
            .find(|(h, _)| *h == self.harness)
            .map(|(_, events)| events.values().filter(|on| **on).count())
            .unwrap_or(0);
        hooks + usize::from(self.mcp && cfg.mcp.iter().any(|(h, on)| *h == self.harness && *on))
    }

    fn total(&self) -> usize {
        self.events.len() + usize::from(self.mcp)
    }

    fn toggle(&self, cfg: &mut OnboardConfig) {
        let on = self.selected(cfg) == 0;
        if let Some((_, events)) = cfg.hooks.iter_mut().find(|(h, _)| *h == self.harness) {
            for value in events.values_mut() {
                *value = on;
            }
        }
        if self.mcp {
            if let Some((_, value)) = cfg.mcp.iter_mut().find(|(h, _)| *h == self.harness) {
                *value = on;
            }
        }
    }
}

fn screen_agents(
    t: &mut Terminal<CrosstermBackend<io::Stdout>>,
    p: &Palette,
    cfg: &mut OnboardConfig,
    allow_mcp: bool,
    remote: &mut Vec<RemoteConnection>,
) -> TResult<Dir> {
    let agents = AgentChoice::choices(cfg, allow_mcp);
    let mut state =
        ListState::default().with_selected(if agents.is_empty() { None } else { Some(0) });
    let mut remote_state = ListState::default().with_selected(Some(0));
    let mut tab = 0usize;
    let mut motion = Motion::new();
    loop {
        let options: &[RemoteConnection] = if tab == 1 {
            &[
                RemoteConnection::Dots,
                RemoteConnection::GrokBot,
                RemoteConnection::Hark,
            ]
        } else {
            &[RemoteConnection::Muse, RemoteConnection::Computer]
        };
        t.draw(|f| {
            if tab == 0 {
                draw_agents(f, p, cfg, &agents, &mut state, &motion);
                if f.area().width >= MIN_COLS && f.area().height >= MIN_ROWS {
                    connection_tabs(f, p, tab);
                    footer(f, p, "1-3 tabs  space select  d details  b back  q quit");
                }
            } else if draw_frame(f, p, 3, "Choose your agents").is_some() {
                let area = f.area();
                connection_tabs(f, p, tab);
                let items = options
                    .iter()
                    .map(|choice| {
                        ListItem::new(Line::from(vec![
                            Span::styled(
                                if remote.contains(choice) {
                                    "[x] "
                                } else {
                                    "[ ] "
                                },
                                p.cyan,
                            ),
                            Span::raw(choice.name()),
                            Span::styled(format!(" / {}", choice.status()), p.amber),
                        ]))
                    })
                    .collect::<Vec<_>>();
                f.render_stateful_widget(
                    List::new(items)
                        .highlight_symbol("> ")
                        .highlight_style(p.cyan),
                    Rect::new(2, 6, area.width - 4, area.height - 11),
                    &mut remote_state,
                );
                put(
                    f,
                    area.height - 5,
                    2,
                    "Selection opens guidance. It does not connect yet.",
                    p.amber,
                );
                primary_action(
                    f,
                    p,
                    Rect::new(2, area.height - 4, area.width - 4, 3),
                    "Review connection setup",
                );
                footer(f, p, "1-3 tabs  space select  d details  b back  q quit");
            }
        })
        .map_err(|_| Quit)?;
        let idx = if tab == 0 {
            state.selected().unwrap_or(0)
        } else {
            remote_state.selected().unwrap_or(0)
        };
        let count = if tab == 0 {
            agents.len()
        } else {
            options.len()
        };
        match motion.read(0.3)? {
            k if is_quit(&k) => return Err(Quit),
            KeyCode::Char(key @ '1'..='3') => {
                tab = key as usize - '1' as usize;
                remote_state.select(Some(0));
            }
            KeyCode::Tab | KeyCode::Right => {
                tab = (tab + 1) % 3;
                remote_state.select(Some(0));
            }
            KeyCode::BackTab | KeyCode::Left => {
                tab = (tab + 2) % 3;
                remote_state.select(Some(0));
            }
            KeyCode::Up | KeyCode::Char('k') if count > 0 => {
                if tab == 0 {
                    state.select(Some((idx + count - 1) % count));
                } else {
                    remote_state.select(Some((idx + count - 1) % count));
                }
                motion.restart();
            }
            KeyCode::Down | KeyCode::Char('j') if count > 0 => {
                if tab == 0 {
                    state.select(Some((idx + 1) % count));
                } else {
                    remote_state.select(Some((idx + 1) % count));
                }
                motion.restart();
            }
            KeyCode::Char(' ') if count > 0 => {
                if tab == 0 {
                    agents[idx].toggle(cfg);
                } else if let Some(pos) = remote.iter().position(|choice| *choice == options[idx]) {
                    remote.remove(pos);
                } else {
                    remote.push(options[idx]);
                }
                motion.restart();
            }
            KeyCode::Char('d') if count > 0 => {
                if tab == 0 {
                    screen_agent_details(t, p, cfg, &agents[idx])?;
                } else {
                    connection_guide(t, p, options[idx])?;
                }
                motion.restart();
            }
            KeyCode::Enter => return Ok(Dir::Forward),
            KeyCode::Char('b') => return Ok(Dir::Back),
            _ => {}
        }
    }
}

fn connection_guide(
    t: &mut Terminal<CrosstermBackend<io::Stdout>>,
    p: &Palette,
    choice: RemoteConnection,
) -> TResult<Dir> {
    pause(
        t,
        p,
        4,
        &format!("{} / {}", choice.name(), choice.status()),
        &choice.guide(),
        true,
        Some("j/k scroll   enter continue   b back   q quit"),
    )
}

fn screen_connections(
    t: &mut Terminal<CrosstermBackend<io::Stdout>>,
    p: &Palette,
    cfg: &OnboardConfig,
    remote: &[RemoteConnection],
    index: &mut usize,
) -> TResult<Dir> {
    let mut guides = Vec::new();
    for agent in AgentChoice::choices(cfg, true)
        .iter()
        .filter(|a| a.selected(cfg) > 0)
    {
        let mut lines = vec![
            format!("Set up {} on this computer", agent.name()),
            String::new(),
            "1. Keep your existing agent account/provider sign-in.".into(),
            "2. Review the selected hooks and Everett tools next.".into(),
            "   Applying will back up and update only selected settings.".into(),
        ];
        if agent.mcp && cfg.mcp.iter().any(|(h, on)| *h == agent.harness && *on) {
            lines.push(format!(
                "   Tools file: {}",
                crate::install::mcp_path(&agent.harness).display()
            ));
        }
        if agent.events.iter().any(|event| {
            cfg.hooks
                .iter()
                .any(|(h, events)| *h == agent.harness && events.get(event) == Some(&true))
        }) {
            lines.push(format!(
                "   Hooks file: {}",
                crate::install::settings_path(&agent.harness).display()
            ));
        }
        lines.extend([
            String::new(),
            "3. After applying, restart this agent so it loads Everett.".into(),
            "4. Ask it to list Everett tools and verify a real result.".into(),
            "5. Copy the final prompt to review an instruction-file addition.".into(),
            String::new(),
            "Nothing on this page signs into the agent's provider.".into(),
            "Instruction Markdown is never edited by this wizard.".into(),
        ]);
        guides.push((format!("{} / setup guide", agent.name()), lines));
    }
    guides.extend(remote.iter().map(|choice| {
        (
            format!("{} / {}", choice.name(), choice.status()),
            choice.guide(),
        )
    }));
    if guides.is_empty() {
        return Ok(Dir::Forward);
    }
    *index = (*index).min(guides.len() - 1);
    loop {
        let (title, lines) = &guides[*index];
        match pause(
            t,
            p,
            4,
            title,
            lines,
            true,
            Some("j/k scroll   enter continue   b back   q quit"),
        )? {
            Dir::Back if *index == 0 => return Ok(Dir::Back),
            Dir::Back => *index -= 1,
            _ if *index + 1 == guides.len() => return Ok(Dir::Forward),
            _ => *index += 1,
        }
    }
}

fn draw_agents(
    f: &mut Frame,
    p: &Palette,
    cfg: &OnboardConfig,
    agents: &[AgentChoice],
    state: &mut ListState,
    motion: &Motion,
) {
    let Some(y) = draw_frame(f, p, 3, "Choose your agents") else {
        return;
    };
    let area = f.area();
    if agents.is_empty() {
        put(
            f,
            y + 1,
            2,
            "No supported agents found on this machine.",
            p.amber,
        );
        put(
            f,
            y + 2,
            2,
            "Install an agent or run its CLI once, then run",
            Style::default(),
        );
        put(f, y + 3, 2, "everett onboard again.", p.cyan);
        put(
            f,
            y + 4,
            2,
            "Cloud and VM choices are in tabs 2 and 3.",
            p.dim,
        );
        primary_action(
            f,
            p,
            Rect::new(2, area.height - 4, area.width - 4, 3),
            "Continue with current choices",
        );
        footer(f, p, "enter continue   b back   q quit");
        return;
    }
    put(
        f,
        y + 1,
        2,
        "Space selects an agent; d reviews its settings.",
        Style::default(),
    );
    let split = area.width >= 76 && area.height >= 20;
    let list_area = if split {
        Rect::new(2, y + 2, area.width / 2 - 3, area.height - y - 7)
    } else {
        Rect::new(2, y + 2, area.width - 4, area.height - y - 9)
    };
    let items: Vec<_> = agents
        .iter()
        .map(|agent| {
            let selected = agent.selected(cfg);
            let (mark, style) = match (selected, agent.total()) {
                (_, 0) => (" - ", p.dim),
                (0, _) => ("[ ]", p.dim),
                (n, total) if n == total => ("[x]", p.green),
                _ => ("[-]", p.amber),
            };
            let plural = if agent.sessions == 1 { "" } else { "s" };
            let line = if split {
                Line::from(vec![
                    Span::styled(format!("{} ", mark), style),
                    Span::raw(agent.name()),
                ])
            } else {
                Line::from(vec![
                    Span::styled(format!("{} ", mark), style),
                    Span::raw(format!(
                        "{:<12} {:>4} session{}{}",
                        agent.name(),
                        agent.sessions,
                        plural,
                        if agent.total() == 0 {
                            " / discovery only"
                        } else {
                            ""
                        }
                    )),
                ])
            };
            if split {
                ListItem::new(vec![
                    line,
                    Line::from(Span::styled(
                        format!(
                            "    {} session{}{}",
                            agent.sessions,
                            plural,
                            if agent.total() == 0 {
                                " / discovery only"
                            } else {
                                ""
                            }
                        ),
                        p.dim,
                    )),
                ])
            } else {
                ListItem::new(line)
            }
        })
        .collect();
    let mut list = List::new(items)
        .highlight_symbol(if p.ascii { "> " } else { "▎ " })
        .highlight_style(p.cyan);
    if split {
        list = list.block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(p.dim)
                .title(Span::styled(" On this machine ", p.dim)),
        );
    }
    f.render_stateful_widget(list, list_area, state);
    let agent = &agents[state.selected().unwrap_or(0)];
    let selected = agent.selected(cfg);
    let (status, status_style) = match (selected, agent.total()) {
        (_, 0) => ("Discovery only. No settings will be changed.", p.amber),
        (0, _) => (
            "Skipped. Existing settings will be left as they are.",
            p.dim,
        ),
        (n, total) if n < total => ("Custom selection. Open details to review.", p.amber),
        _ => ("Selected. Changes are reviewed before applying.", p.green),
    };
    if split {
        let panel = Rect::new(
            area.width / 2 + 1,
            y + 2,
            area.width - area.width / 2 - 3,
            list_area.height,
        );
        let pulse = motion.elapsed() < 0.3;
        f.render_widget(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(if pulse { p.cyan } else { p.violet })
                .title(Span::styled(" On apply ", p.violet)),
            panel,
        );
        let x = panel.x + 2;
        put(f, panel.y + 2, x, agent.name(), p.cyan);
        let state_label = match (selected, agent.total()) {
            (_, 0) => "DISCOVERY ONLY",
            (0, _) => "SKIPPED",
            (n, total) if n < total => "CUSTOM",
            _ => "SELECTED",
        };
        put(f, panel.y + 3, x, state_label, status_style);
        let mut features = Vec::new();
        if selected == 0 {
            features.push(Line::from(Span::styled(
                if agent.total() == 0 {
                    "Sessions stay discoverable. Configure tools manually."
                } else {
                    "Existing settings stay as they are. Sessions remain discoverable."
                },
                p.dim,
            )));
        } else {
            if agent.mcp && cfg.mcp.iter().any(|(h, on)| *h == agent.harness && *on) {
                features.push(Line::from(Span::styled("Everett tools", p.violet)));
            }
            if let Some((_, events)) = cfg.hooks.iter().find(|(h, _)| *h == agent.harness) {
                if events.get("SessionStart").copied().unwrap_or(false)
                    || events.get("extension").copied().unwrap_or(false)
                {
                    features.push(Line::from(Span::styled("Share session context", p.cyan)));
                }
                if events.get("Stop").copied().unwrap_or(false) {
                    features.push(Line::from(Span::styled("Save session cards", p.accent)));
                }
                if ["UserPromptSubmit", "PostToolUse", "extension"]
                    .iter()
                    .any(|event| events.get(*event).copied().unwrap_or(false))
                {
                    features.push(Line::from(Span::styled("Deliver messages", p.green)));
                }
            }
        }
        f.render_widget(
            Paragraph::new(features).wrap(Wrap { trim: false }),
            Rect::new(
                x,
                panel.y + 5,
                panel.width - 4,
                panel.height.saturating_sub(7),
            ),
        );
        if pulse {
            let span = panel.width.saturating_sub(4);
            let pos = (motion.elapsed() / 0.3 * span as f64) as u16;
            put(
                f,
                panel.y + panel.height - 2,
                x + pos.min(span.saturating_sub(1)),
                if p.ascii { "+" } else { "◆" },
                p.accent,
            );
        }
        put(f, area.height - 5, 2, status, status_style);
    } else {
        put(f, area.height - 6, 2, status, status_style);
        let purpose = match (!agent.events.is_empty(), agent.mcp) {
            (true, true) => "Adds Everett tools and automatic session updates.",
            (true, false) => "Adds automatic session updates. Tool setup is off.",
            (false, true) => "Adds Everett tools to this agent.",
            _ => "Sessions stay discoverable; configure tools manually.",
        };
        put(f, area.height - 5, 2, purpose, p.violet);
    }
    let selected_agents = agents
        .iter()
        .filter(|agent| agent.selected(cfg) > 0)
        .count();
    let next = if selected_agents == 0 {
        "Continue without integrations".to_string()
    } else {
        format!(
            "Continue with {} agent{}",
            selected_agents,
            if selected_agents == 1 { "" } else { "s" }
        )
    };
    primary_action(
        f,
        p,
        Rect::new(2, area.height - 4, area.width - 4, 3),
        &next,
    );
    footer(f, p, "up/down move   b back   q quit");
}

type BoolSetter = Box<dyn Fn(&mut OnboardConfig, bool)>;

struct CheckItem {
    label: String,
    path: String,
    get: Box<dyn Fn(&OnboardConfig) -> bool>,
    set: BoolSetter,
}

fn screen_agent_details(
    t: &mut Terminal<CrosstermBackend<io::Stdout>>,
    p: &Palette,
    cfg: &mut OnboardConfig,
    agent: &AgentChoice,
) -> TResult<()> {
    let mut items = Vec::new();
    let path = if agent.harness == "omp" {
        crate::install::omp_extension_path()
    } else {
        crate::install::settings_path(&agent.harness)
    };
    for event in &agent.events {
        let (h_get, h_set) = (agent.harness.clone(), agent.harness.clone());
        let (e_get, e_set) = (event.clone(), event.clone());
        items.push(CheckItem {
            label: match event.as_str() {
                "SessionStart" => "Share context when a session starts",
                "Stop" => "Save a session card when a task stops",
                "UserPromptSubmit" => "Receive messages at the next prompt",
                "PostToolUse" => "Receive messages after tool calls",
                "extension" => "Session context and live message delivery",
                other => other,
            }
            .to_string(),
            path: path.display().to_string(),
            get: Box::new(move |c| {
                c.hooks
                    .iter()
                    .find(|(h, _)| *h == h_get)
                    .and_then(|(_, events)| events.get(&e_get))
                    .copied()
                    .unwrap_or(false)
            }),
            set: Box::new(move |c, on| {
                if let Some((_, events)) = c.hooks.iter_mut().find(|(h, _)| *h == h_set) {
                    events.insert(e_set.clone(), on);
                }
            }),
        });
    }
    if agent.mcp {
        let (h_get, h_set) = (agent.harness.clone(), agent.harness.clone());
        items.push(CheckItem {
            label: "Everett tools (MCP server)".to_string(),
            path: crate::install::mcp_path(&agent.harness)
                .display()
                .to_string(),
            get: Box::new(move |c| c.mcp.iter().any(|(h, on)| *h == h_get && *on)),
            set: Box::new(move |c, on| {
                if let Some((_, value)) = c.mcp.iter_mut().find(|(h, _)| *h == h_set) {
                    *value = on;
                }
            }),
        });
    }
    let mut state =
        ListState::default().with_selected(if items.is_empty() { None } else { Some(0) });
    loop {
        t.draw(|f| {
            let Some(y) = draw_frame(f, p, 3, &format!("{} / setup details", agent.name())) else {
                return;
            };
            let area = f.area();
            put(
                f,
                y,
                2,
                "Nothing is written until the final apply step.",
                p.dim,
            );
            if items.is_empty() {
                put(
                    f,
                    y + 2,
                    2,
                    "No automatic changes enabled for this agent.",
                    p.amber,
                );
            } else {
                let rows: Vec<_> = items
                    .iter()
                    .map(|item| {
                        ListItem::new(format!(
                            "{} {}",
                            glyph((item.get)(cfg), p.ascii),
                            item.label
                        ))
                    })
                    .collect();
                f.render_stateful_widget(
                    List::new(rows)
                        .highlight_symbol("> ")
                        .highlight_style(Style::default().add_modifier(Modifier::REVERSED)),
                    Rect::new(2, y + 2, area.width - 4, area.height - y - 9),
                    &mut state,
                );
                let item = &items[state.selected().unwrap_or(0)];
                f.render_widget(
                    Paragraph::new(format!("File changed on apply:\n{}", item.path))
                        .style(p.dim)
                        .wrap(Wrap { trim: false }),
                    Rect::new(2, area.height - 6, area.width - 4, 4),
                );
            }
            footer(f, p, "up/down move   space toggle   enter/b back   q quit");
        })
        .map_err(|_| Quit)?;
        let idx = state.selected().unwrap_or(0);
        match read_key()? {
            k if is_quit(&k) => return Err(Quit),
            KeyCode::Up | KeyCode::Char('k') if !items.is_empty() => {
                state.select(Some((idx + items.len() - 1) % items.len()))
            }
            KeyCode::Down | KeyCode::Char('j') if !items.is_empty() => {
                state.select(Some((idx + 1) % items.len()))
            }
            KeyCode::Char(' ') if !items.is_empty() => {
                let on = (items[idx].get)(cfg);
                (items[idx].set)(cfg, !on);
            }
            KeyCode::Enter | KeyCode::Char('b') => return Ok(()),
            _ => {}
        }
    }
}

/// The input stays masked. Escape or Ctrl-C cancels; printable characters remain input.
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
                footer(f, p, "type key   enter submit / empty skip   esc cancel");
            }
        })
        .map_err(|_| Quit)?;
        match read_key()? {
            KeyCode::Esc => return Err(Quit),
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
            if let Some(y) = draw_frame(f, p, 5, "Smarter routing / optional") {
                put(
                    f,
                    y,
                    2,
                    "Local matching works without a routing key.",
                    p.dim,
                );
                put(
                    f,
                    y + 2,
                    2,
                    &format!(
                        "{} Jev key{}",
                        glyph(cfg.jev_choice == "paste", p.ascii),
                        if cfg.jev_key.is_empty() {
                            " (p to enter)"
                        } else {
                            " entered"
                        }
                    ),
                    p.violet,
                );
                put(
                    f,
                    y + 3,
                    2,
                    &format!(
                        "{} Local matching",
                        glyph(cfg.jev_choice == "skip", p.ascii)
                    ),
                    p.cyan,
                );
                put(
                    f,
                    y + 5,
                    2,
                    &format!(
                        "Nightly merge: {}{}",
                        if cfg.trunk_schedule_enabled {
                            "on"
                        } else {
                            "off"
                        },
                        if cfg!(target_os = "macos") {
                            " (04:00, Claude)"
                        } else {
                            " (requires macOS)"
                        }
                    ),
                    p.dim,
                );
                footer(f, p, "p key  s local  m nightly  enter/b back  q quit");
            }
        })
        .map_err(|_| Quit)?;
        match read_key()? {
            k if is_quit(&k) => return Err(Quit),
            KeyCode::Char('p') => {
                let entered = text_entry(
                    t,
                    p,
                    4,
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
                    if let Some(y) = draw_frame(f, p, 4, "Smarter routing (optional)") {
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
                        4,
                        "Smarter routing (optional)",
                        &["ok ✓ -- the key works.".to_string()],
                        false,
                        Some("enter continue"),
                    )?;
                } else {
                    let keep = loop {
                        t.draw(|f| {
                            if let Some(y) = draw_frame(f, p, 4, "Smarter routing (optional)") {
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
            if let Some(y) = draw_frame(f, p, 5, "Backfill cards / optional") {
                put(f, y, 2, "Summarize older sessions into local cards.", p.dim);
                put(
                    f,
                    y + 1,
                    2,
                    &format!("{} enabled", glyph(cfg.backfill_enabled, p.ascii)),
                    p.cyan,
                );
                put(
                    f,
                    y + 3,
                    2,
                    &format!("Span: {}", stepper_text(cfg.span_days, "day")),
                    p.accent,
                );
                put(
                    f,
                    y + 4,
                    2,
                    &format!("{} session(s) without a card", preview_len),
                    p.dim,
                );
                put(f, y + 5, 2, "Deterministic cards. No model calls.", p.dim);
                footer(f, p, "space toggle  h/l days  enter/b back  q quit");
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

fn screen_advanced(
    t: &mut Terminal<CrosstermBackend<io::Stdout>>,
    p: &Palette,
    cfg: &mut OnboardConfig,
) -> TResult<()> {
    loop {
        t.draw(|f| {
            if let Some(y) = draw_frame(f, p, 5, "Advanced / optional") {
                put(f, y, 2, "1  Routing key and nightly memory merge", p.violet);
                put(f, y + 2, 2, "2  Cards for older sessions", p.cyan);
                put(
                    f,
                    y + 4,
                    2,
                    "These are optional. Local matching works now.",
                    p.dim,
                );
                primary_action(
                    f,
                    p,
                    Rect::new(2, f.area().height - 4, f.area().width - 4, 3),
                    "Return to review",
                );
                footer(f, p, "1 routing   2 cards   enter/b review   q quit");
            }
        })
        .map_err(|_| Quit)?;
        match read_key()? {
            key if is_quit(&key) => return Err(Quit),
            KeyCode::Char('1') => {
                screen_jev(t, p, cfg)?;
            }
            KeyCode::Char('2') => {
                screen_backfill(t, p, cfg)?;
            }
            KeyCode::Enter | KeyCode::Char('b') => return Ok(()),
            _ => {}
        }
    }
}

fn screen_confirm(
    t: &mut Terminal<CrosstermBackend<io::Stdout>>,
    p: &Palette,
    cfg: &mut OnboardConfig,
    remote: &[RemoteConnection],
) -> TResult<Dir> {
    let mut scroll = 0u16;
    let mut max_scroll = 0u16;
    loop {
        t.draw(|f| {
            if let Some(y) = draw_frame(f, p, 5, "Review your setup") {
                let area = f.area();
                let mut lines = vec![Line::from(Span::styled("Local changes on apply", p.cyan))];
                let agents = AgentChoice::choices(cfg, true);
                let mut count = 0;
                for agent in &agents {
                    if agent.selected(cfg) > 0 {
                        count += 1;
                        let hooks = agent.events.iter().any(|event| {
                            cfg.hooks.iter().any(|(h, events)| {
                                *h == agent.harness && events.get(event) == Some(&true)
                            })
                        });
                        let tools =
                            agent.mcp && cfg.mcp.iter().any(|(h, on)| *h == agent.harness && *on);
                        let changes = match (hooks, tools) {
                            (true, true) => "hooks + tools",
                            (true, false) => "hooks",
                            _ => "tools",
                        };
                        lines.push(Line::from(format!("  {} / {}", agent.name(), changes)));
                    }
                }
                if count == 0 {
                    lines.push(Line::from("  No local integrations selected."));
                }
                for choice in remote {
                    lines.push(Line::from(Span::styled(
                        format!("{} / pending, not added", choice.name()),
                        p.amber,
                    )));
                }
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    format!(
                        "Older cards: {} / nightly merge: {}",
                        if cfg.backfill_enabled { "on" } else { "off" },
                        if cfg.trunk_schedule_enabled {
                            "on"
                        } else {
                            "off"
                        }
                    ),
                    p.dim,
                )));
                lines.push(Line::from(Span::styled(
                    if cfg.jev_choice == "paste" && !cfg.jev_key.is_empty() {
                        "Routing: save entered Jev key"
                    } else {
                        "Routing: keep existing settings"
                    },
                    p.dim,
                )));
                lines.push(Line::from(Span::styled(
                    "d file details / v advanced",
                    p.dim,
                )));
                let height = area.height - y - 5;
                max_scroll = lines.len().saturating_sub(height as usize) as u16;
                let paragraph = Paragraph::new(lines);
                scroll = scroll.min(max_scroll);
                f.render_widget(
                    paragraph.scroll((scroll, 0)),
                    Rect::new(2, y, area.width - 4, height),
                );
                if max_scroll > 0 {
                    put(
                        f,
                        area.height - 5,
                        2,
                        "j/k scroll to review every choice",
                        p.dim,
                    );
                }
                primary_action(
                    f,
                    p,
                    Rect::new(2, area.height - 4, area.width - 4, 3),
                    "Apply local changes",
                );
                footer(f, p, "d details   v advanced   b back   q quit");
            }
        })
        .map_err(|_| Quit)?;
        match read_key()? {
            key if is_quit(&key) => return Err(Quit),
            KeyCode::Enter | KeyCode::Char('a') => return Ok(Dir::Apply),
            KeyCode::Char('b') => return Ok(Dir::Back),
            KeyCode::Char('d') => {
                pause(
                    t,
                    p,
                    5,
                    "Files and optional changes",
                    &confirm_lines(cfg),
                    true,
                    Some("j/k scroll   enter/b review   q quit"),
                )?;
            }
            KeyCode::Char('v') => {
                screen_advanced(t, p, cfg)?;
            }
            KeyCode::Down | KeyCode::Char('j') => scroll = (scroll + 1).min(max_scroll),
            KeyCode::Up | KeyCode::Char('k') => scroll = scroll.saturating_sub(1),
            KeyCode::End => scroll = max_scroll,
            KeyCode::Home => scroll = 0,
            _ => {}
        }
    }
}

fn screen_apply(
    t: &mut Terminal<CrosstermBackend<io::Stdout>>,
    p: &Palette,
    cfg: &OnboardConfig,
) -> TResult<onboard::ApplyResult> {
    t.draw(|f| {
        if let Some(y) = draw_frame(f, p, 5, "Applying local changes") {
            put(f, y, 2, "Writing the settings you reviewed...", p.cyan);
        }
    })
    .map_err(|_| Quit)?;
    let term = std::cell::RefCell::new(t);
    let progress = |done: usize, total: usize| {
        let _ = term.borrow_mut().draw(|f| {
            if let Some(y) = draw_frame(f, p, 5, "Applying local changes") {
                put(
                    f,
                    y,
                    2,
                    &format!("Session cards: {} / {}", done, total),
                    p.cyan,
                );
            }
        });
    };
    Ok(onboard::run_apply(cfg, Some(&progress)))
}

fn screen_summary(
    t: &mut Terminal<CrosstermBackend<io::Stdout>>,
    p: &Palette,
    cfg: &OnboardConfig,
    result: &onboard::ApplyResult,
    remote: &[RemoteConnection],
) -> TResult<bool> {
    let local_changes = cfg
        .hooks
        .iter()
        .any(|(_, events)| events.values().any(|on| *on))
        || cfg.mcp.iter().any(|(_, on)| *on)
        || cfg.backfill_enabled
        || cfg.trunk_schedule_enabled
        || cfg.jev_choice == "paste";
    let tools = AgentChoice::choices(cfg, true)
        .into_iter()
        .find(|agent| agent.mcp && cfg.mcp.iter().any(|(h, on)| *h == agent.harness && *on));
    loop {
        t.draw(|f| {
            let title = if !result.errors.is_empty() {
                "Setup needs attention"
            } else if local_changes {
                "Local setup applied"
            } else if !remote.is_empty() {
                "Remote setup pending"
            } else {
                "No local changes selected"
            };
            if let Some(y) = draw_frame(f, p, 5, title) {
                let area = f.area();
                put(
                    f,
                    y,
                    2,
                    if !result.errors.is_empty() {
                        "Some settings failed. Press d for exact errors."
                    } else if local_changes {
                        "Settings saved. Agent tools are not verified yet."
                    } else {
                        "No cloud client or VM connection was added."
                    },
                    if result.errors.is_empty() {
                        p.green
                    } else {
                        p.amber
                    },
                );
                put(
                    f,
                    y + 1,
                    2,
                    &format!("Remote setup pending: {}", remote.len()),
                    p.dim,
                );
                let next = if !result.errors.is_empty() {
                    "Run: everett doctor".to_string()
                } else if let Some(agent) = &tools {
                    format!("Restart {}. Ask it to list Everett tools.", agent.name())
                } else if let Some(choice) = remote.first() {
                    format!("Press d for {} setup steps.", choice.name())
                } else {
                    "Run: everett ls".to_string()
                };
                f.render_widget(
                    Paragraph::new(next)
                        .style(p.cyan)
                        .wrap(Wrap { trim: false }),
                    Rect::new(2, y + 3, area.width - 4, area.height - y - 7),
                );
                primary_action(
                    f,
                    p,
                    Rect::new(2, area.height - 4, area.width - 4, 3),
                    if result.errors.is_empty() {
                        "Get your agent instruction prompt"
                    } else {
                        "Finish"
                    },
                );
                footer(f, p, "d results   enter continue   q exit");
            }
        })
        .map_err(|_| Quit)?;
        match read_key()? {
            KeyCode::Char('d') => {
                let mut lines = onboard::summary_lines(cfg, result);
                if !result.errors.is_empty() {
                    lines.insert(0, "Needs attention:".into());
                    lines.splice(1..1, result.errors.iter().cloned());
                }
                if let Some(index) = lines.iter().position(|line| line == "Try next:") {
                    lines.truncate(index);
                }
                for choice in remote {
                    lines.push(format!("{}: pending; no client grant added", choice.name()));
                    lines.extend(choice.guide());
                }
                if pause(
                    t,
                    p,
                    5,
                    "Setup results",
                    &lines,
                    true,
                    Some("j/k scroll   enter/b return   q exit"),
                )
                .is_err()
                {
                    return Ok(false);
                }
            }
            key if is_quit(&key) => return Ok(false),
            KeyCode::Enter => return Ok(true),
            _ => {}
        }
    }
}

fn screen_instructions(
    t: &mut Terminal<CrosstermBackend<io::Stdout>>,
    p: &Palette,
    prompt: &str,
) -> TResult<()> {
    let mut scroll = 0u16;
    let mut max_scroll = 0u16;
    loop {
        t.draw(|f| {
            if let Some(y) = draw_frame(f, p, 6, "Teach your agents about Everett") {
                let area = f.area();
                put(
                    f,
                    y,
                    2,
                    "Copy this prompt into each selected agent.",
                    p.cyan,
                );
                let width = area.width.saturating_sub(6).max(1) as usize;
                let chars: Vec<char> = prompt.chars().collect();
                let rows: Vec<Line> = chars
                    .chunks(width)
                    .map(|part| Line::from(part.iter().collect::<String>()))
                    .collect();
                max_scroll = rows
                    .len()
                    .saturating_sub(area.height.saturating_sub(y + 8) as usize)
                    as u16;
                scroll = scroll.min(max_scroll);
                f.render_widget(
                    Paragraph::new(rows).scroll((scroll, 0)).block(
                        Block::default()
                            .borders(Borders::ALL)
                            .title(" Prompt / review before applying "),
                    ),
                    Rect::new(2, y + 2, area.width - 4, area.height - y - 6),
                );
                primary_action(
                    f,
                    p,
                    Rect::new(2, area.height - 4, area.width - 4, 3),
                    "Print prompt and finish",
                );
                footer(f, p, "j/k scroll   enter/q finish");
            }
        })
        .map_err(|_| Quit)?;
        match read_key()? {
            KeyCode::Enter | KeyCode::Esc | KeyCode::Char('q') => return Ok(()),
            KeyCode::Down | KeyCode::Char('j') => scroll = scroll.saturating_add(1).min(max_scroll),
            KeyCode::Up | KeyCode::Char('k') => scroll = scroll.saturating_sub(1),
            KeyCode::Home => scroll = 0,
            KeyCode::End => scroll = max_scroll,
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

/// A cancelled wizard returns None. Applied results retain write failures for the caller.
pub fn run(
    args: &crate::cli::Args,
    mut cfg: OnboardConfig,
) -> Result<Option<onboard::ApplyResult>, std::io::Error> {
    use std::io::IsTerminal;
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "terminal unavailable",
        ));
    }
    match std::env::var("TERM") {
        Ok(t) if t != "dumb" => {}
        _ => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "terminal unavailable",
            ))
        }
    }
    enable_raw_mode()?;
    let _guard = TermGuard;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, cursor::Hide)?;
    let backend = CrosstermBackend::new(stdout);
    let mut term = Terminal::new(backend)?;
    term.clear()?;
    let p = Palette::new();

    cfg.span_days = args.span_days;
    cfg.backfill_enabled = false;
    cfg.trunk_schedule_enabled = args.schedule_merge;
    let mut remote = Vec::new();
    let mut connection_index = 0usize;
    if args.no_mcp {
        cfg.mcp = cfg.mcp.iter().map(|(h, _)| (h.clone(), false)).collect();
    }

    let mut i = 0usize;
    while i < 5 {
        let dir = match i {
            0 => screen_welcome(&mut term, &p),
            1 => screen_account(&mut term, &p),
            2 => screen_agents(&mut term, &p, &mut cfg, !args.no_mcp, &mut remote),
            3 => screen_connections(&mut term, &p, &cfg, &remote, &mut connection_index),
            _ => screen_confirm(&mut term, &p, &mut cfg, &remote),
        };
        match dir {
            Err(Quit) => return Ok(None),
            Ok(Dir::Back) => {
                i = i.saturating_sub(1);
            }
            Ok(Dir::Apply) => match screen_apply(&mut term, &p, &cfg) {
                Err(Quit) => return Ok(None),
                Ok(mut result) => {
                    let show_prompt =
                        screen_summary(&mut term, &p, &cfg, &result, &remote).unwrap_or(false);
                    if result.errors.is_empty() {
                        let mut agents: Vec<String> = AgentChoice::choices(&cfg, true)
                            .iter()
                            .filter(|agent| agent.selected(&cfg) > 0)
                            .map(|agent| agent.name().to_string())
                            .collect();
                        agents.extend(remote.iter().map(|choice| choice.name().to_string()));
                        result.instruction_prompt = onboard::instruction_prompt(&agents);
                        if show_prompt {
                            let _ = screen_instructions(&mut term, &p, &result.instruction_prompt);
                        }
                    }
                    return Ok(Some(result));
                }
            },
            Ok(Dir::Forward) => {
                if i == 2 {
                    connection_index = 0;
                }
                i += 1;
            }
        }
    }
    Ok(None)
}
