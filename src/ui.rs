use anyhow::Result;
use crossterm::{
    cursor::{Hide, Show},
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use fuzzy_matcher::{skim::SkimMatcherV2, FuzzyMatcher};
#[allow(clippy::wildcard_imports)]
use ratatui::{prelude::*, widgets::*};
use std::{
    cell::RefCell,
    cmp::min,
    io,
    rc::Rc,
};
use tui_input::backend::crossterm::EventHandler;
use tui_input::Input;
use unicode_width::UnicodeWidthStr;

use crate::{searchable::Searchable, ssh, theme::Theme};

const PROMPT: &str = "❯ ";
const SELECTION_MARKER: &str = "▌ ";
const SELECTION_PADDING: &str = "  ";

#[derive(Clone)]
pub struct AppConfig {
    pub config_paths: Vec<String>,

    /// When true, every entry in `config_paths` must resolve to a readable
    /// file (user supplied `--config` explicitly). When false, missing files
    /// are silently ignored (auto-discovered defaults).
    pub strict_missing: bool,

    pub search_filter: Option<String>,
    pub sort_by_name: bool,
    pub show_proxy_command: bool,

    pub command_template: String,
    pub command_template_on_session_start: Option<String>,
    pub command_template_on_session_end: Option<String>,
    pub exit_after_ssh_session_ends: bool,
}

pub struct App {
    config: AppConfig,
    theme: Theme,
    matcher: SkimMatcherV2,

    search: Input,

    table_state: TableState,
    hosts: Searchable<ssh::Host>,
    table_columns_constraints: Vec<Constraint>,
}

#[derive(PartialEq)]
enum AppKeyAction {
    Ok,
    Stop,
    Continue,
}

impl App {
    /// # Errors
    ///
    /// Will return `Err` if the SSH configuration file cannot be parsed.
    pub fn new(config: &AppConfig) -> Result<App> {
        let mut hosts = Vec::new();

        let expanded = ssh::expand_config_paths(&config.config_paths);
        for path in &expanded {
            let parsed_hosts = match ssh::parse_config(path) {
                Ok(h) => h,
                Err(err) => {
                    // Missing files are tolerated for auto-discovered defaults.
                    // The system-wide config is always optional, even under
                    // strict mode, to preserve existing behaviour.
                    let is_missing = matches!(
                        &err,
                        ssh::ParseConfigError::Io(io_err)
                            if io_err.kind() == std::io::ErrorKind::NotFound
                    );
                    let is_system_default =
                        path.as_os_str() == std::ffi::OsStr::new("/etc/ssh/ssh_config");

                    if is_missing && (!config.strict_missing || is_system_default) {
                        continue;
                    }

                    anyhow::bail!(
                        "Failed to parse SSH configuration file {}: {err:?}",
                        path.display()
                    );
                }
            };

            hosts.extend(parsed_hosts);
        }

        // ソート (host.name の文字列で)
        if config.sort_by_name {
            hosts.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        }

        // 検索バーの初期入力
        let search_input = config.search_filter.clone().unwrap_or_default();
        let matcher = SkimMatcherV2::default();

        // Searchable に格納
        let mut app = App {
            config: config.clone(),
            theme: Theme::dark(),
            matcher: SkimMatcherV2::default(),

            search: search_input.clone().into(),

            table_state: TableState::default().with_selected(0),
            table_columns_constraints: vec![],
            hosts: Searchable::new(
                hosts,
                &search_input,
                move |host: &&ssh::Host, search_value: &str| -> bool {
                    search_value.is_empty()
                        || matcher.fuzzy_match(&host.name, search_value).is_some()
                        || matcher
                            .fuzzy_match(&host.destination, search_value)
                            .is_some()
                        || matcher.fuzzy_match(&host.aliases, search_value).is_some()
                },
            ),
        };
        app.calculate_table_columns_constraints();

        Ok(app)
    }

    /// # Errors
    ///
    /// Will return `Err` if the terminal cannot be configured.
    pub fn start(&mut self) -> Result<()> {
        let stdout = io::stdout().lock();
        let backend = CrosstermBackend::new(stdout);
        let terminal = Rc::new(RefCell::new(Terminal::new(backend)?));

        setup_terminal(&terminal)?;

        // create app and run it
        let res = self.run(&terminal);

        restore_terminal(&terminal)?;

        if let Err(err) = res {
            println!("{err:?}");
        }

        Ok(())
    }

    fn run<B>(&mut self, terminal: &Rc<RefCell<Terminal<B>>>) -> Result<()>
    where
        B: Backend + std::io::Write,
    {
        loop {
            terminal.borrow_mut().draw(|f| ui(f, self))?;

            let ev = event::read()?;

            if let Event::Key(key) = ev {
                if key.kind == KeyEventKind::Press {
                    let action = self.on_key_press(terminal, key)?;
                    match action {
                        AppKeyAction::Ok => continue,
                        AppKeyAction::Stop => break,
                        AppKeyAction::Continue => {}
                    }
                }

                // 入力が検索バーに反映される
                self.search.handle_event(&ev);
                self.hosts.search(self.search.value());

                let selected = self.table_state.selected().unwrap_or(0);
                if selected >= self.hosts.len() {
                    self.table_state.select(Some(match self.hosts.len() {
                        0 => 0,
                        _ => self.hosts.len() - 1,
                    }));
                }
            }
        }

        Ok(())
    }

    fn on_key_press<B>(
        &mut self,
        terminal: &Rc<RefCell<Terminal<B>>>,
        key: KeyEvent,
    ) -> Result<AppKeyAction>
    where
        B: Backend + std::io::Write,
    {
        #[allow(clippy::enum_glob_use)]
        use KeyCode::*;

        let is_ctrl_pressed = key.modifiers.contains(KeyModifiers::CONTROL);

        if is_ctrl_pressed {
            let action = self.on_key_press_ctrl(key);
            if action != AppKeyAction::Continue {
                return Ok(action);
            }
        }

        match key.code {
            Esc => return Ok(AppKeyAction::Stop),
            Down => self.next(),
            Up => self.previous(),
            Home => self.table_state.select(Some(0)),
            End => {
                if !self.hosts.is_empty() {
                    self.table_state.select(Some(self.hosts.len() - 1));
                }
            }
            PageDown => {
                let i = self.table_state.selected().unwrap_or(0);
                let target = min(i.saturating_add(21), self.hosts.len().saturating_sub(1));

                self.table_state.select(Some(target));
            }
            PageUp => {
                let i = self.table_state.selected().unwrap_or(0);
                let target = i.saturating_sub(21);

                self.table_state.select(Some(target));
            }
            Enter => {
                let selected = self.table_state.selected().unwrap_or(0);
                if selected >= self.hosts.len() {
                    return Ok(AppKeyAction::Ok);
                }

                let host: &ssh::Host = &self.hosts[selected];

                restore_terminal(terminal).expect("Failed to restore terminal");

                if let Some(template) = &self.config.command_template_on_session_start {
                    host.run_command_template(template)?;
                }

                host.run_command_template(&self.config.command_template)?;

                if let Some(template) = &self.config.command_template_on_session_end {
                    host.run_command_template(template)?;
                }

                setup_terminal(terminal).expect("Failed to setup terminal");

                if self.config.exit_after_ssh_session_ends {
                    return Ok(AppKeyAction::Stop);
                }
            }
            _ => return Ok(AppKeyAction::Continue),
        }

        Ok(AppKeyAction::Ok)
    }

    fn on_key_press_ctrl(&mut self, key: KeyEvent) -> AppKeyAction {
        #[allow(clippy::enum_glob_use)]
        use KeyCode::*;

        match key.code {
            Char('c') => AppKeyAction::Stop,
            Char('j' | 'n') => {
                self.next();
                AppKeyAction::Ok
            }
            Char('k' | 'p') => {
                self.previous();
                AppKeyAction::Ok
            }
            _ => AppKeyAction::Continue,
        }
    }

    fn next(&mut self) {
        let i = match self.table_state.selected() {
            Some(i) => {
                if self.hosts.is_empty() || i >= self.hosts.len() - 1 {
                    0
                } else {
                    i + 1
                }
            }
            None => 0,
        };
        self.table_state.select(Some(i));
    }

    fn previous(&mut self) {
        let i = match self.table_state.selected() {
            Some(i) => {
                if self.hosts.is_empty() {
                    0
                } else if i == 0 {
                    self.hosts.len() - 1
                } else {
                    i - 1
                }
            }
            None => 0,
        };
        self.table_state.select(Some(i));
    }

    fn calculate_table_columns_constraints(&mut self) {
        let mut lengths = Vec::new();

        let name_len = self
            .hosts
            .iter()
            .map(|d| d.name.as_str())
            .map(UnicodeWidthStr::width)
            .max()
            .unwrap_or(0);
        lengths.push(name_len);

        let aliases_len = self
            .hosts
            .non_filtered_iter()
            .map(|d| d.aliases.as_str())
            .map(UnicodeWidthStr::width)
            .max()
            .unwrap_or(0);
        lengths.push(aliases_len);

        let user_len = self
            .hosts
            .non_filtered_iter()
            .map(|d| match &d.user {
                Some(u) => u.as_str(),
                None => "",
            })
            .map(UnicodeWidthStr::width)
            .max()
            .unwrap_or(0);
        lengths.push(user_len);

        let destination_len = self
            .hosts
            .non_filtered_iter()
            .map(|d| d.destination.as_str())
            .map(UnicodeWidthStr::width)
            .max()
            .unwrap_or(0);
        lengths.push(destination_len);

        let port_len = self
            .hosts
            .non_filtered_iter()
            .map(|d| match &d.port {
                Some(port) => port.as_str(),
                None => "",
            })
            .map(UnicodeWidthStr::width)
            .max()
            .unwrap_or(0);
        lengths.push(port_len);

        if self.config.show_proxy_command {
            let proxy_len = self
                .hosts
                .non_filtered_iter()
                .map(|d| match &d.proxy_command {
                    Some(p) => p.as_str(),
                    None => "",
                })
                .map(UnicodeWidthStr::width)
                .max()
                .unwrap_or(0);
            lengths.push(proxy_len);
        }

        let mut new_constraints = vec![
            // Marker column (▌ / spaces) — width matches SELECTION_MARKER.
            Constraint::Length(u16::try_from(UnicodeWidthStr::width(SELECTION_MARKER)).unwrap_or(2)),
            // Name column (+1 for breathing room).
            Constraint::Length(u16::try_from(lengths[0]).unwrap_or_default() + 1),
        ];
        new_constraints.extend(
            lengths
                .iter()
                .skip(1)
                .map(|len| Constraint::Min(u16::try_from(*len).unwrap_or_default() + 1)),
        );

        self.table_columns_constraints = new_constraints;
    }
}

fn setup_terminal<B>(terminal: &Rc<RefCell<Terminal<B>>>) -> Result<()>
where
    B: Backend + std::io::Write,
{
    let mut terminal = terminal.borrow_mut();

    // setup terminal
    enable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        Hide,
        EnterAlternateScreen,
        EnableMouseCapture
    )?;

    Ok(())
}

fn restore_terminal<B>(terminal: &Rc<RefCell<Terminal<B>>>) -> Result<()>
where
    B: Backend + std::io::Write,
{
    let mut terminal = terminal.borrow_mut();
    terminal.clear()?;

    // restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        Show,
        LeaveAlternateScreen,
        DisableMouseCapture,
    )?;

    Ok(())
}

/// メインの描画関数
fn ui(f: &mut Frame, app: &mut App) {
    let layout_main = Layout::vertical([
        Constraint::Length(3),  // search bar (single line + borders)
        Constraint::Min(6),     // host table (fills available space)
        Constraint::Length(8),  // detail panel
        Constraint::Length(1),  // footer (single-line, no border)
    ])
    .split(f.area());

    render_searchbar(f, app, layout_main[0]);
    render_table(f, app, layout_main[1]);
    render_detail_panel(f, app, layout_main[2]);
    render_footer(f, app, layout_main[3]);

    // Place cursor inside the search bar (1 line border + PROMPT width).
    let prompt_width = u16::try_from(UnicodeWidthStr::width(PROMPT)).unwrap_or(2);
    let mut cursor_position = layout_main[0].as_position();
    cursor_position.x += u16::try_from(app.search.cursor()).unwrap_or_default() + prompt_width + 1;
    cursor_position.y += 1;
    f.set_cursor_position(cursor_position);
}

fn render_searchbar(f: &mut Frame, app: &mut App, area: Rect) {
    let theme = &app.theme;
    let prompt = Span::styled(PROMPT, Style::default().fg(theme.accent).add_modifier(Modifier::BOLD));
    let query = Span::styled(app.search.value(), Style::default().fg(theme.text));
    let content = Line::from(vec![prompt, query]);

    let matched = app.hosts.len();
    let total = app.hosts.total_len();
    let count = format!(" {matched} / {total} ");
    let title_right = Line::from(Span::styled(
        count,
        Style::default().fg(theme.muted),
    ))
    .right_aligned();

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.border_focused))
        .title(Line::from(Span::styled(
            " Search ",
            Style::default().fg(theme.primary).add_modifier(Modifier::BOLD),
        )))
        .title(title_right);

    let paragraph = Paragraph::new(content).block(block);
    f.render_widget(paragraph, area);
}

fn render_table(f: &mut Frame, app: &mut App, area: Rect) {
    let theme = &app.theme;
    let query = app.search.value().to_string();

    // First column is the marker, then the existing data columns.
    let mut header_cells: Vec<Cell> = vec![Cell::from("")];
    let data_headers = if app.config.show_proxy_command {
        vec!["NAME", "ALIASES", "USER", "DESTINATION", "PORT", "PROXY"]
    } else {
        vec!["NAME", "ALIASES", "USER", "DESTINATION", "PORT"]
    };
    header_cells.extend(
        data_headers
            .iter()
            .map(|h| Cell::from(Span::styled(*h, theme.header_style()))),
    );

    let header = Row::new(header_cells).height(1).bottom_margin(1);
    let selected_idx = app.table_state.selected().unwrap_or(usize::MAX);

    let rows = app
        .hosts
        .iter()
        .enumerate()
        .map(|(idx, host)| build_row(idx, selected_idx, host, &query, &app.matcher, theme, app.config.show_proxy_command))
        .collect::<Vec<_>>();

    let block = Block::default()
        .borders(Borders::NONE)
        .padding(Padding::horizontal(1));

    let table = Table::new(rows, &app.table_columns_constraints)
        .header(header)
        .row_highlight_style(theme.selection_style())
        .column_spacing(2)
        .block(block);

    f.render_stateful_widget(table, area, &mut app.table_state);
}

fn build_row<'a>(
    idx: usize,
    selected_idx: usize,
    host: &'a ssh::Host,
    query: &str,
    matcher: &SkimMatcherV2,
    theme: &Theme,
    show_proxy: bool,
) -> Row<'a> {
    let marker = if idx == selected_idx {
        Cell::from(Span::styled(
            SELECTION_MARKER,
            Style::default()
                .fg(theme.selection_marker)
                .add_modifier(Modifier::BOLD),
        ))
    } else {
        Cell::from(SELECTION_PADDING)
    };

    let name_cell = highlighted_cell(&host.name, query, matcher, theme);
    let aliases_cell = Cell::from(Span::styled(
        host.aliases.clone(),
        Style::default().fg(theme.text_dim),
    ));
    let user_cell = Cell::from(Span::styled(
        host.user.clone().unwrap_or_default(),
        Style::default().fg(theme.text_dim),
    ));
    let destination_cell = highlighted_cell(&host.destination, query, matcher, theme);
    let port_cell = Cell::from(Span::styled(
        host.port.clone().unwrap_or_default(),
        Style::default().fg(theme.text_dim),
    ));

    let mut cells = vec![marker, name_cell, aliases_cell, user_cell, destination_cell, port_cell];
    if show_proxy {
        cells.push(Cell::from(Span::styled(
            host.proxy_command.clone().unwrap_or_default(),
            Style::default().fg(theme.text_dim),
        )));
    }

    Row::new(cells)
}

fn highlighted_cell<'a>(
    value: &'a str,
    query: &str,
    matcher: &SkimMatcherV2,
    theme: &Theme,
) -> Cell<'a> {
    let base = Style::default().fg(theme.text);
    if query.is_empty() {
        return Cell::from(Span::styled(value.to_string(), base));
    }

    let Some((_, indices)) = matcher.fuzzy_indices(value, query) else {
        return Cell::from(Span::styled(value.to_string(), base));
    };

    let highlight = theme.match_style();
    let mut spans = Vec::new();
    let mut buf = String::new();
    let mut buf_is_match: Option<bool> = None;

    for (i, ch) in value.chars().enumerate() {
        let is_match = indices.contains(&i);
        match buf_is_match {
            Some(prev) if prev == is_match => buf.push(ch),
            Some(prev) => {
                spans.push(Span::styled(
                    std::mem::take(&mut buf),
                    if prev { highlight } else { base },
                ));
                buf.push(ch);
                buf_is_match = Some(is_match);
            }
            None => {
                buf.push(ch);
                buf_is_match = Some(is_match);
            }
        }
    }

    if let Some(prev) = buf_is_match {
        spans.push(Span::styled(buf, if prev { highlight } else { base }));
    }

    Cell::from(Line::from(spans))
}

/// 詳細パネル: 選択中のホストの ProxyJump / ProxyCommand / IdentityFile /
/// LocalForward を key:value 表示する。値が空の項目は省略する。
fn render_detail_panel(f: &mut Frame, app: &mut App, area: Rect) {
    let theme = &app.theme;
    let selected_index = app.table_state.selected().unwrap_or(0);

    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(theme.border_style())
        .title(Line::from(Span::styled(
            " Host detail ",
            Style::default().fg(theme.primary).add_modifier(Modifier::BOLD),
        )))
        .padding(Padding::horizontal(2));

    if app.hosts.is_empty() || selected_index >= app.hosts.len() {
        let paragraph = Paragraph::new(Span::styled(
            "No host selected",
            Style::default().fg(theme.muted),
        ))
        .block(block);
        f.render_widget(paragraph, area);
        return;
    }

    let host = &app.hosts[selected_index];
    let mut lines: Vec<Line> = Vec::new();

    let mut push_field = |label: &str, value: &str| {
        if value.is_empty() {
            return;
        }
        lines.push(Line::from(vec![
            Span::styled(
                format!("{label:<14}"),
                Style::default().fg(theme.muted).add_modifier(Modifier::BOLD),
            ),
            Span::styled(value.to_string(), Style::default().fg(theme.text)),
        ]));
    };

    push_field("Hostname", &host.destination);
    if let Some(v) = host.user.as_deref() {
        push_field("User", v);
    }
    if let Some(v) = host.port.as_deref() {
        push_field("Port", v);
    }
    if let Some(v) = host.proxy_jump.as_deref() {
        push_field("ProxyJump", v);
    }
    if let Some(v) = host.proxy_command.as_deref() {
        push_field("ProxyCommand", v);
    }
    if let Some(v) = host.identity_file.as_deref() {
        push_field("IdentityFile", v);
    }

    if !host.local_forwards.is_empty() {
        let first = host.local_forwards.first().unwrap();
        let formatted = format!("{} → {}:{}", first.local_port, first.remote_host, first.remote_port);
        push_field("LocalForward", &formatted);
        let indent = " ".repeat(14);
        for lf in host.local_forwards.iter().skip(1) {
            lines.push(Line::from(vec![
                Span::raw(indent.clone()),
                Span::styled(
                    format!("{} → {}:{}", lf.local_port, lf.remote_host, lf.remote_port),
                    Style::default().fg(theme.text),
                ),
            ]));
        }
    }

    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "(no extra settings)",
            Style::default().fg(theme.muted),
        )));
    }

    let paragraph = Paragraph::new(lines).block(block);
    f.render_widget(paragraph, area);
}

fn render_footer(f: &mut Frame, app: &mut App, area: Rect) {
    let theme = &app.theme;
    let sep = Span::styled("  │  ", Style::default().fg(theme.border));

    let chips = [
        ("↑↓", "navigate"),
        ("↵", "connect"),
        ("⌫", "edit"),
        ("esc", "quit"),
    ];

    let mut spans: Vec<Span> = Vec::new();
    spans.push(Span::raw(" "));
    for (i, (key, label)) in chips.iter().enumerate() {
        if i > 0 {
            spans.push(sep.clone());
        }
        spans.push(Span::styled(
            format!(" {key} "),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            (*label).to_string(),
            Style::default().fg(theme.muted),
        ));
    }

    let paragraph = Paragraph::new(Line::from(spans));
    f.render_widget(paragraph, area);
}
