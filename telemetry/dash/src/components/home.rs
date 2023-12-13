use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
    time::Duration,
};

use itertools::Itertools;

use crossterm::event::{KeyCode, KeyEvent};
use eyre::Result;
use ratatui::{prelude::*, widgets::*};
use serde::{Deserialize, Serialize};
use telemetry::{ChannelEvent, ClientId, Id, TelemetryEvent};
use tokio::sync::{
    mpsc::{self, UnboundedSender},
    Mutex,
};

use super::{Component, Frame};
use crate::{
    action::Action,
    config::{Config, KeyBindings},
    tui::Event,
};

struct ChannelState {
    name: String,
    capacity: usize,
    max_capacity: usize,
}

struct ClientState {
    channels: HashMap<Id, ChannelState>,
    deadline: std::time::SystemTime,
}

type State = HashMap<ClientId, ClientState>;

pub struct Home {
    command_tx: Option<UnboundedSender<Action>>,
    config: Config,
    events: VecDeque<(ClientId, TelemetryEvent)>,
    clients: State,
    client_tab: usize,
    // stream: Arc<Mutex<Option>>,
    stream: Option<mpsc::Receiver<(ClientId, TelemetryEvent)>>,
}

impl Home {
    pub fn new() -> Self {
        Self {
            command_tx: None,
            config: Config::default(),
            events: VecDeque::new(),
            clients: State::default(),
            client_tab: 0,
            stream: None,
        }
    }

    async fn update_inner(&mut self, action: Action) -> Result<Option<Action>> {
        fn client_deadline() -> std::time::SystemTime {
            std::time::SystemTime::now() + std::time::Duration::from_secs(10)
        }

        match action {
            Action::Tick => {
                if let Some(stream) = self.stream.as_mut() {
                    const MAX_EVENTS_PER_TICK: usize = 10;
                    let mut events_count = 0;
                    while let Ok((client_id, event)) = stream.try_recv() {
                        self.events.push_front((client_id, event.clone()));
                        if self.events.len() > 50 {
                            self.events.pop_back();
                        }
                        if let Some(client) = self.clients.get_mut(&client_id) {
                            // update deadline because we received something from client
                            client.deadline = client_deadline();
                            match event {
                                TelemetryEvent::ChannelStatistics(statistics) => {
                                    if let Some(channel) = client.channels.get_mut(&statistics.id) {
                                        channel.capacity = statistics.capacity;
                                        channel.max_capacity = statistics.max_capacity;
                                    } else {
                                        client.channels.insert(
                                            statistics.id,
                                            ChannelState {
                                                capacity: statistics.capacity,
                                                max_capacity: statistics.max_capacity,
                                                name: format!("<unknown {}>", statistics.id),
                                            },
                                        );
                                    }
                                }
                                TelemetryEvent::ChannelEvent(ChannelEvent::Open(id, name)) => {
                                    client.channels.insert(
                                        id,
                                        ChannelState {
                                            name: name,
                                            capacity: 1,
                                            max_capacity: 1,
                                        },
                                    );
                                }
                                TelemetryEvent::ChannelEvent(ChannelEvent::Close(id)) => {
                                    client.channels.remove(&id);
                                }
                                TelemetryEvent::New => {}
                            }
                        } else {
                            self.clients.insert(
                                client_id,
                                ClientState {
                                    channels: Default::default(),
                                    deadline: client_deadline(),
                                },
                            );
                        }

                        events_count += 1;
                        if events_count > MAX_EVENTS_PER_TICK {
                            break;
                        }
                    }
                    let mut remove_ids = vec![];
                    for (id, client) in self.clients.iter() {
                        if let Ok(_) = client.deadline.elapsed() {
                            remove_ids.push(*id);
                        }
                    }
                    for id in remove_ids {
                        self.clients.remove(&id);
                    }
                } else {
                    self.stream = Some(telemetry::server::stream().await);
                }
            }
            _ => {}
        }
        Ok(None)
    }

    fn draw_inner(&mut self, f: &mut Frame<'_>, area: Rect) -> Result<()> {
        f.render_widget(Paragraph::new("remote dash"), area);

        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(0),
                Constraint::Max(5),
            ])
            .split(area.inner(&Margin::new(1, 1)));

        {
            let area = layout[0];
            let tabs = Tabs::new(self.clients.keys().map(|k| format!("{k}")).collect())
                .block(Block::default().title("clients").borders(Borders::ALL))
                .select(self.client_tab)
                .highlight_style(Style::new().bold().underlined());
            f.render_widget(tabs, area);
        }

        {
            let area = layout[1];
            if let Some(client) = self.clients.values().skip(self.client_tab).next() {
                if client.channels.len() > 0 {
                    const GAUGE_HEIGHT: u16 = 1;
                    let splits = (((client.channels.len() * GAUGE_HEIGHT as usize) as f64)
                        / area.height as f64) as u16
                        + 1;

                    let items_per_split = client.channels.len() / splits as usize;

                    let layout = Layout::default()
                        .direction(Direction::Horizontal)
                        .constraints(
                            (0..splits)
                                .map(|_| Constraint::Percentage((100 / splits) as u16))
                                .collect::<Vec<_>>(),
                        )
                        .split(area);

                    let max_name_len = client
                        .channels
                        .values()
                        .map(|c| c.name.len())
                        .fold(1, |x, y| if x > y { x } else { y });

                    for (area, chunk) in layout
                        .iter()
                        .zip(&client.channels.iter().chunks(items_per_split))
                    {
                        let layout = Layout::default()
                            .direction(Direction::Vertical)
                            .constraints(
                                client
                                    .channels
                                    .iter()
                                    .map(|_| Constraint::Max(GAUGE_HEIGHT))
                                    .collect::<Vec<_>>(),
                            )
                            .split(*area);

                        for (area, (id, channel)) in layout.iter().zip(chunk) {
                            let ratio = channel.capacity as f64 / channel.max_capacity as f64;
                            let color = match ratio {
                                0.0..=0.3 => Style::new().red(),
                                0.3..=0.6 => Style::new().yellow(),
                                _ => Style::new().green(),
                            };
                            let layout = Layout::default()
                                .direction(Direction::Horizontal)
                                .constraints([
                                    Constraint::Length(max_name_len as u16 + 2),
                                    Constraint::Min(0),
                                    Constraint::Length(2),
                                ])
                                .split(*area);

                            f.render_widget(Paragraph::new(channel.name.clone()), layout[0]);
                            f.render_widget(
                                Gauge::default().gauge_style(color).ratio(ratio),
                                layout[1],
                            )
                        }
                    }
                }
            }
        }

        {
            let area = layout[2];

            let mut list_items: Vec<ListItem> = vec![];

            for (client_id, event) in self.events.iter() {
                list_items.push(ListItem::new(format!("{} {:?}", client_id, event)));
            }

            let w =
                List::new(list_items).block(Block::default().title("Events").borders(Borders::ALL));

            f.render_widget(w, area);
        }

        Ok(())
    }

    async fn handle_events_inner(&mut self, event: Option<Event>) -> Result<Option<Action>> {
        match event {
            Some(Event::Key(key)) => {
                if key.code == KeyCode::Tab {
                    self.client_tab += 1;
                    if self.client_tab >= self.clients.len() {
                        self.client_tab = 0;
                    }
                }
            }

            _ => {}
        };

        Ok(None)
    }
}

#[async_trait::async_trait]
impl Component for Home {
    async fn handle_events(&mut self, event: Option<Event>) -> Result<Option<Action>> {
        self.handle_events_inner(event).await
    }

    async fn register_action_handler(&mut self, tx: UnboundedSender<Action>) -> Result<()> {
        self.command_tx = Some(tx);
        Ok(())
    }

    async fn register_config_handler(&mut self, config: Config) -> Result<()> {
        self.config = config;
        Ok(())
    }

    async fn update(&mut self, action: Action) -> Result<Option<Action>> {
        self.update_inner(action).await
    }

    fn draw(&mut self, f: &mut Frame<'_>, area: Rect) -> Result<()> {
        self.draw_inner(f, area)
    }
}
