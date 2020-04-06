use crate::event::{Event, Events};
use std::{collections::BTreeMap, io::Write};
use termion::{
    cursor::Goto,
    event::Key,
    input::MouseTerminal,
    raw::{IntoRawMode, RawTerminal},
    screen::AlternateScreen,
};
use tui::{
    backend::TermionBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Style},
    widgets::{Block, Borders, List, Paragraph, Tabs, Text},
    Terminal,
};
use unicode_width::UnicodeWidthStr;

use probe_rs_rtt::{DownChannel, UpChannel};

struct ChannelState {
    up_channel: UpChannel,
    down_channel: Option<DownChannel>,
    messages: Vec<String>,
    last_line_done: bool,
    input: String,
    scroll_offset: usize,
    rtt_buffer: [u8; 1024],
}

impl ChannelState {
    pub fn new(up_channel: UpChannel, down_channel: Option<DownChannel>) -> Self {
        Self {
            up_channel,
            down_channel,
            messages: Vec::new(),
            last_line_done: false,
            input: String::new(),
            scroll_offset: 0,
            rtt_buffer: [0u8; 1024],
        }
    }

    /// Polls the RTT target for new data on the specified channel.
    ///
    /// Processes all the new data and adds it to the linebuffer of the respective channel.
    fn poll_rtt(&mut self) {
        // TODO: Proper error handling.
        let count = match self.up_channel.read(self.rtt_buffer.as_mut()) {
            Ok(count) => count,
            Err(err) => {
                eprintln!("\nError reading from RTT: {}", err);
                return;
            }
        };

        if count == 0 {
            return;
        }

        // First, convert the incomming bytes to UTF8.
        let mut incomming = String::from_utf8_lossy(&self.rtt_buffer[..count]).to_string();

        // Then pop the last stored line from our line buffer if possible and append our new line.
        if !self.last_line_done {
            if let Some(last_line) = self.messages.pop() {
                incomming = last_line + &incomming;
            }
        }
        self.last_line_done = incomming.chars().last().unwrap() == '\n';

        // Then split the entire new contents.
        let split = incomming.split_terminator('\n');

        // Then add all the splits to the linebuffer.
        self.messages.extend(split.clone().map(|s| s.to_string()));

        if self.scroll_offset != 0 {
            self.scroll_offset += split.count();
        }
    }

    pub fn push_rtt(&mut self) {
        if let Some(down_channel) = self.down_channel.as_mut() {
            self.input += "\n";
            down_channel.write(&self.input.as_bytes()).unwrap();
            self.input.clear();
        }
    }
}

/// App holds the state of the application
pub struct App {
    tabs: Vec<ChannelState>,
    current_tab: usize,

    terminal:
        Terminal<TermionBackend<AlternateScreen<MouseTerminal<RawTerminal<std::io::Stdout>>>>>,
    events: Events,
}

impl App {
    pub fn new(mut channels: (BTreeMap<usize, UpChannel>, BTreeMap<usize, DownChannel>)) -> Self {
        let stdout = std::io::stdout().into_raw_mode().unwrap();
        let stdout = MouseTerminal::from(stdout);
        let stdout = AlternateScreen::from(stdout);
        let backend = TermionBackend::new(stdout);
        let terminal = Terminal::new(backend).unwrap();

        let events = Events::new();

        let mut tabs = Vec::with_capacity(channels.0.len());

        for (n, channel) in channels.0 {
            tabs.push(ChannelState::new(channel, channels.1.remove(&n)));
        }

        Self {
            tabs,
            current_tab: 0,

            terminal,
            events,
        }
    }

    pub fn render(&mut self) {
        let input = self.tabs[self.current_tab].input.clone();
        let has_down_channel = self.tabs[self.current_tab].down_channel.is_some();
        let scroll_offset = self.tabs[self.current_tab].scroll_offset;
        let message_num = self.tabs[self.current_tab].messages.len();
        let messages = self.tabs[self.current_tab].messages.iter();
        let tabs = &self.tabs;
        let current_tab = self.current_tab;
        let mut height = 0;
        self.terminal
            .draw(|mut f| {
                let constraints = if has_down_channel {
                    &[
                        Constraint::Length(1),
                        Constraint::Min(1),
                        Constraint::Length(1),
                    ][..]
                } else {
                    &[Constraint::Length(1), Constraint::Min(1)][..]
                };
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .margin(0)
                    .constraints(constraints)
                    .split(f.size());

                let tab_names = tabs
                    .iter()
                    .map(|t| t.up_channel.name().unwrap_or("Unnamed Channel"))
                    .collect::<Vec<_>>();
                let mut tabs = Tabs::default()
                    .titles(&tab_names.as_slice())
                    .select(current_tab)
                    .style(Style::default().fg(Color::Black).bg(Color::Magenta))
                    .highlight_style(Style::default().fg(Color::Yellow).bg(Color::Magenta));
                f.render(&mut tabs, chunks[0]);

                height = chunks[1].height as usize;

                let messages = messages
                    .map(|m| Text::raw(m))
                    .skip(message_num - (height + scroll_offset).min(message_num))
                    .take(height);
                let mut messages =
                    List::new(messages).block(Block::default().borders(Borders::NONE));
                f.render(&mut messages, chunks[1]);

                if has_down_channel {
                    let text = [Text::raw(input.clone())];
                    let mut input = Paragraph::new(text.iter())
                        .style(Style::default().fg(Color::Yellow).bg(Color::Blue));
                    f.render(&mut input, chunks[2]);
                }
            })
            .unwrap();

        let message_num = self.tabs[self.current_tab].messages.len();
        let scroll_offset = self.tabs[self.current_tab].scroll_offset;
        if message_num < height + scroll_offset {
            self.tabs[self.current_tab].scroll_offset = message_num - height.min(message_num);
        }

        if has_down_channel {
            // Put the cursor back inside the input box
            let height = self.terminal.size().map(|s| s.height).unwrap_or(1);
            write!(
                self.terminal.backend_mut(),
                "{}",
                Goto(input.width() as u16 + 1, height)
            )
            .unwrap();
            // stdout is buffered, flush it to see the effect immediately when hitting backspace
            std::io::stdout().flush().ok();
        }
    }

    /// Returns true if the application should exit.
    pub fn handle_event(&mut self) -> bool {
        match self.events.next().unwrap() {
            Event::Input(input) => match input {
                Key::Ctrl('c') => true,
                Key::F(n) => {
                    let n = n as usize - 1;
                    if n < self.tabs.len() {
                        self.current_tab = n as usize;
                    }
                    false
                }
                Key::Char('\n') => {
                    self.push_rtt();
                    false
                }
                Key::Char(c) => {
                    self.tabs[self.current_tab].input.push(c);
                    false
                }
                Key::Backspace => {
                    self.tabs[self.current_tab].input.pop();
                    false
                }
                Key::PageUp => {
                    self.tabs[self.current_tab].scroll_offset += 1;
                    false
                }
                Key::PageDown => {
                    if self.tabs[self.current_tab].scroll_offset > 0 {
                        self.tabs[self.current_tab].scroll_offset -= 1;
                    }
                    false
                }
                _ => false,
            },
            _ => false,
        }
    }

    /// Polls the RTT target for new data on all channels.
    pub fn poll_rtt(&mut self) {
        for channel in &mut self.tabs {
            channel.poll_rtt();
        }
    }

    pub fn push_rtt(&mut self) {
        self.tabs[self.current_tab].push_rtt();
    }
}
