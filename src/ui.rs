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
    cmp::{max, min},
    io,
    rc::Rc,
};
use tui_input::backend::crossterm::EventHandler;
use tui_input::Input;
use unicode_width::UnicodeWidthStr;

use crate::{searchable::Searchable, ssh};

const INFO_TEXT: &str = "(Esc) quit | (↑) move up | (↓) move down | (enter) select";

#[derive(Clone)]
pub struct AppConfig {
    pub config_paths: Vec<String>,

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

        for path in &config.config_paths {
            let parsed_hosts = match ssh::parse_config(path) {
                Ok(h) => h,
                Err(err) => {
                    if path == "/etc/ssh/ssh_config" {
                        if let ssh::ParseConfigError::Io(io_err) = &err {
                            // Ignore missing system-wide SSH configuration file
                            if io_err.kind() == std::io::ErrorKind::NotFound {
                                continue;
                            }
                        }
                    }

                    anyhow::bail!("Failed to parse SSH configuration file: {err:?}");
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
            // +1 for padding
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
    // (上)検索バー, (中段上)テーブル, (中段下)ローカルフォワード, (下)フッター という4分割
    let layout_main = Layout::vertical([
        Constraint::Length(3),  // search bar
        Constraint::Length(10), // table
        Constraint::Min(3),     // local forward panel
        Constraint::Length(3),  // footer
    ])
    .split(f.area());

    render_searchbar(f, app, layout_main[0]);
    render_table(f, app, layout_main[1]);
    render_local_forwards_panel(f, app, layout_main[2]);
    render_footer(f, app, layout_main[3]);

    // 検索バーにカーソル表示
    let mut cursor_position = layout_main[0].as_position();
    cursor_position.x += u16::try_from(app.search.cursor()).unwrap_or_default() + 4;
    cursor_position.y += 1;
    f.set_cursor_position(cursor_position);
}

fn render_searchbar(f: &mut Frame, app: &mut App, area: Rect) {
    let info_footer = Paragraph::new(Line::from(app.search.value())).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .padding(Padding::horizontal(3))
            .title("Search"),
    );
    f.render_widget(info_footer, area);
}

fn render_table(f: &mut Frame, app: &mut App, area: Rect) {
    let header_style = Style::default().fg(Color::Cyan);
    let selected_style = Style::default().add_modifier(Modifier::REVERSED);

    let mut header_names = vec!["Name", "Aliases", "User", "Destination", "Port"];
    if app.config.show_proxy_command {
        header_names.push("Proxy");
    }

    let header = Row::new(header_names.iter().map(|h| Cell::from(*h)))
        .style(header_style)
        .height(1);

    let rows = app.hosts.iter().map(|host| {
        let mut content = vec![
            host.name.clone(),
            host.aliases.clone(),
            host.user.clone().unwrap_or_default(),
            host.destination.clone(),
            host.port.clone().unwrap_or_default(),
        ];
        if app.config.show_proxy_command {
            content.push(host.proxy_command.clone().unwrap_or_default());
        }

        Row::new(content)
    });

    let t = Table::new(rows, &app.table_columns_constraints)
        .header(header)
        .row_highlight_style(selected_style)
        .block(Block::default().borders(Borders::ALL).title("Hosts"));

    f.render_stateful_widget(t, area, &mut app.table_state);
}

/// LocalForward を複数行で描画
fn render_local_forwards_panel(f: &mut Frame, app: &mut App, area: Rect) {
    let selected_index = app.table_state.selected().unwrap_or(0);
    if app.hosts.is_empty() || selected_index >= app.hosts.len() {
        let empty_par = Paragraph::new("No host selected").block(
            Block::default()
                .borders(Borders::ALL)
                .title("Local Forwards")
                .border_type(BorderType::Rounded),
        );
        f.render_widget(empty_par, area);
        return;
    }

    let host = &app.hosts[selected_index];

    // LocalForward を複数行テキストにする
    let mut lines = vec![];
    if host.local_forwards.is_empty() {
        lines.push("No LocalForward defined".to_string());
    } else {
        for lf in &host.local_forwards {
            let line = format!("{} -> {}:{}", lf.local_port, lf.remote_host, lf.remote_port);
            lines.push(line);
        }
    }

    let lines = lines.into_iter().map(Line::from).collect::<Vec<_>>();

    let paragraph = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .title("Local Forwards")
            .border_type(BorderType::Rounded),
    );

    f.render_widget(paragraph, area);
}

fn render_footer(f: &mut Frame, _app: &mut App, area: Rect) {
    let info_footer = Paragraph::new(Line::from(INFO_TEXT))
        .alignment(Alignment::Center)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .title("Help"),
        );
    f.render_widget(info_footer, area);
}
