use crate::util::event::{Event, Events};
use std::io::Write;
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
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, List, Paragraph, Tabs, Text},
    Terminal,
};
use unicode_width::UnicodeWidthStr;

use probe_rs_rtt::{Rtt, RttChannel};

struct ChannelState {
    name: String,
    number: usize,
    has_down_channel: bool,
    messages: Vec<String>,
    input: String,
    scroll_offset: usize,
}

impl ChannelState {
    pub fn new(name: impl Into<String>, number: usize, has_down_channel: bool) -> Self {
        Self {
            name: name.into(),
            number,
            has_down_channel,
            messages: Vec::new(),
            input: String::new(),
            scroll_offset: 0,
        }
    }
}

/// App holds the state of the application
pub struct App<'a> {
    tabs: Vec<ChannelState>,
    current_tab: usize,

    terminal:
        Terminal<TermionBackend<AlternateScreen<MouseTerminal<RawTerminal<std::io::Stdout>>>>>,
    events: Events,

    rtt: Rtt<'a>,
    rtt_buffer: [u8; 1024],
}

impl<'a> App<'a> {
    pub fn new(rtt: Rtt<'a>, channels: (Vec<usize>, Vec<usize>)) -> Self {
        let stdout = std::io::stdout().into_raw_mode().unwrap();
        let stdout = MouseTerminal::from(stdout);
        let stdout = AlternateScreen::from(stdout);
        let backend = TermionBackend::new(stdout);
        let terminal = Terminal::new(backend).unwrap();

        let events = Events::new();

        let mut tabs = Vec::with_capacity(channels.0.len());

        for channel in channels.0 {
            tabs.push(ChannelState::new(
                rtt.up_channels()[&channel].name().unwrap_or("Unknown Name"),
                channel,
                channels.1.contains(&channel),
            ));
        }

        Self {
            tabs,
            current_tab: 0,

            terminal,
            events,

            rtt,
            rtt_buffer: [0u8; 1024],
        }
    }

    pub fn render(&mut self) {
        let input = self.tabs[self.current_tab].input.clone();
        let has_down_channel = self.tabs[self.current_tab].has_down_channel;
        let scroll_offset = self.tabs[self.current_tab].scroll_offset;
        let message_num = self.tabs[self.current_tab].messages.len();
        let messages = self.tabs[self.current_tab].messages.iter().enumerate();
        let tabs = &self.tabs;
        let current_tab = self.current_tab;
        let mut height = 0;
        self.terminal
            .draw(|mut f| {
                let constraints = if has_down_channel {
                    &[
                        Constraint::Length(1),
                        Constraint::Length(3),
                        Constraint::Min(1),
                        Constraint::Length(3),
                    ][..]
                } else {
                    &[
                        Constraint::Length(1),
                        Constraint::Length(3),
                        Constraint::Min(1),
                    ][..]
                };
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .margin(0)
                    .constraints(constraints)
                    .split(f.size());

                let text = [Text::raw("ctrl + c: quit — F-keys: switch channels")];
                let mut help_message = Paragraph::new(text.iter());
                f.render(&mut help_message, chunks[0]);

                let tab_names = tabs.iter().map(|t| t.name.clone()).collect::<Vec<_>>();
                let mut tabs = Tabs::default()
                    .block(Block::default().borders(Borders::ALL).title("Channel"))
                    .titles(&tab_names.as_slice())
                    .select(current_tab)
                    .style(Style::default().fg(Color::Cyan))
                    .highlight_style(
                        Style::default()
                            .fg(Color::Yellow)
                            .modifier(Modifier::UNDERLINED),
                    );
                f.render(&mut tabs, chunks[1]);

                height = chunks[2].height as usize - 2;

                let messages = messages
                    .map(|(i, m)| Text::raw(format!("{}: {}", i, m)))
                    .skip(message_num - (height + scroll_offset).min(message_num))
                    .take(height);
                let mut messages = List::new(messages)
                    .block(Block::default().borders(Borders::ALL).title("Messages"));
                f.render(&mut messages, chunks[2]);

                if has_down_channel {
                    let text = [Text::raw(input.clone())];
                    let mut input = Paragraph::new(text.iter())
                        .style(Style::default().fg(Color::Yellow))
                        .block(Block::default().borders(Borders::ALL).title("Input"));
                    f.render(&mut input, chunks[3]);
                }
            })
            .unwrap();

        let message_num = self.tabs[self.current_tab].messages.len();
        let scroll_offset = self.tabs[self.current_tab].scroll_offset;
        if message_num < height + scroll_offset {
            self.tabs[self.current_tab].scroll_offset = message_num - height.min(message_num);
        }

        // Put the cursor back inside the input box
        let height = self.terminal.size().map(|s| s.height).unwrap_or(1);
        write!(
            self.terminal.backend_mut(),
            "{}",
            Goto(2 + input.width() as u16, height - 1)
        )
        .unwrap();
        // stdout is buffered, flush it to see the effect immediately when hitting backspace
        std::io::stdout().flush().ok();
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

    /// Polls the RTT target for new data on the specified channel.
    ///
    /// Processes all the new data and adds it to the linebuffer of the respective channel.
    pub fn read_rtt_channel(&mut self, channel: usize) {
        // TODO: Proper error handling.
        let count = match self.rtt.read(channel, self.rtt_buffer.as_mut()) {
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
        if let Some(last_line) = self.tabs[self.current_tab].messages.pop() {
            incomming = last_line + &incomming;
        }

        // Then split the entire new contents.
        let split = incomming.split('\n');

        // Then add all the splits to the linebuffer.
        self.tabs
            .iter_mut()
            .find(|t| t.number == channel)
            .unwrap()
            .messages
            .extend(split.clone().map(|s| s.to_string()));
        if self.tabs[self.current_tab].scroll_offset != 0 {
            self.tabs[self.current_tab].scroll_offset += split.count() - 1;
        }
    }

    /// Polls the RTT target for new data on all channels.
    pub fn poll_rtt(&mut self) {
        let tabs = self.tabs.iter().map(|c| c.number).collect::<Vec<_>>();
        for channel in tabs {
            if channel == 2 {
                self.read_rtt_channel(channel);
            }
        }
    }

    pub fn push_rtt(&mut self) {
        if self.tabs[self.current_tab].has_down_channel {
            self.rtt
                .write(
                    self.tabs[self.current_tab].number,
                    &self.tabs[self.current_tab].input.as_bytes(),
                )
                .unwrap();
            self.tabs[self.current_tab].input.clear();
        }
    }
}
