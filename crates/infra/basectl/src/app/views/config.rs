use anyhow::Result;
use base_common_genesis::SystemConfig;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    prelude::*,
    widgets::{Block, Borders, Paragraph},
};
use tokio::sync::oneshot;

use crate::{
    app::{Action, Resources, View},
    output::COLOR_BASE_BLUE,
    rpc::fetch_full_system_config,
    tui::{Keybinding, Toast},
};

const KEYBINDINGS: &[Keybinding] = &[
    Keybinding { key: "Esc", description: "Back to home" },
    Keybinding { key: "?", description: "Toggle help" },
    Keybinding { key: "r", description: "Refresh config" },
];

/// View displaying chain configuration and L1 system config parameters.
#[derive(Debug)]
pub struct ConfigView {
    refresh_rx: Option<oneshot::Receiver<Result<SystemConfig>>>,
}

impl Default for ConfigView {
    fn default() -> Self {
        Self::new()
    }
}

impl ConfigView {
    /// Creates a new config view.
    pub const fn new() -> Self {
        Self { refresh_rx: None }
    }
}

impl View for ConfigView {
    fn keybindings(&self) -> &'static [Keybinding] {
        KEYBINDINGS
    }

    fn handle_key(&mut self, key: KeyEvent, resources: &mut Resources) -> Action {
        if key.code == KeyCode::Char('r') && self.refresh_rx.is_none() {
            let l1_rpc = resources.config.l1_rpc.to_string();
            let system_config_addr = resources.config.system_config;
            let (tx, rx) = oneshot::channel();
            self.refresh_rx = Some(rx);
            resources.toasts.push(Toast::info("Refreshing system config…".to_string()));

            tokio::spawn(async move {
                let _ = tx.send(fetch_full_system_config(&l1_rpc, system_config_addr).await);
            });
        }
        Action::None
    }

    fn tick(&mut self, resources: &mut Resources) -> Action {
        let outcome = {
            let Some(rx) = self.refresh_rx.as_mut() else {
                return Action::None;
            };
            match rx.try_recv() {
                Ok(result) => Some(Ok(result)),
                Err(oneshot::error::TryRecvError::Empty) => None,
                Err(oneshot::error::TryRecvError::Closed) => Some(Err(())),
            }
        };

        match outcome {
            Some(Ok(Ok(system_config))) => {
                self.refresh_rx = None;
                resources.system_config = Some(system_config);
                resources.toasts.push(Toast::info("System config refreshed".to_string()));
            }
            Some(Ok(Err(error))) => {
                self.refresh_rx = None;
                resources
                    .toasts
                    .push(Toast::warning(format!("System config refresh failed: {error}")));
            }
            Some(Err(())) => {
                self.refresh_rx = None;
                resources
                    .toasts
                    .push(Toast::warning("System config refresh task dropped".to_string()));
            }
            None => {}
        }

        Action::None
    }

    fn render(&mut self, frame: &mut Frame<'_>, area: Rect, resources: &Resources) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(area);

        render_chain_config(frame, chunks[0], resources);
        render_system_config(frame, chunks[1], resources);
    }
}

fn render_chain_config(f: &mut Frame<'_>, area: Rect, resources: &Resources) {
    let config = &resources.config;

    let batcher_str = config
        .batcher_address
        .as_ref()
        .map(|a| format!("{a:#x}"))
        .unwrap_or_else(|| "-".to_string());

    let mut lines = vec![
        Line::from(vec![
            Span::styled("Chain: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                &config.name,
                Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
    ];

    let fields: &[(&str, &str)] = &[
        ("Active EL RPC", config.rpc.as_str()),
        ("Configured EL RPC", resources.configured_rpc.as_str()),
        ("Public EL RPC", config.public_rpc.as_ref().map_or("-", |url| url.as_str())),
        ("Flashblocks WS", config.flashblocks_ws.as_str()),
        ("L1 RPC", config.l1_rpc.as_str()),
        (
            "Consensus Node RPC",
            config.consensus_node_rpc.as_ref().map(|u| u.as_str()).unwrap_or("-"),
        ),
        ("Batcher Address", &batcher_str),
    ];

    for (label, value) in fields {
        lines.push(Line::from(vec![
            Span::styled(format!("{label}: "), Style::default().fg(Color::DarkGray)),
            Span::styled(*value, Style::default().fg(Color::Cyan)),
        ]));
    }

    let block = Block::default()
        .title(" Chain Configuration ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(COLOR_BASE_BLUE));

    let para = Paragraph::new(lines).block(block);
    f.render_widget(para, area);
}

fn render_system_config(f: &mut Frame<'_>, area: Rect, resources: &Resources) {
    let block = Block::default()
        .title(" L1 SystemConfig ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(COLOR_BASE_BLUE));

    let content = resources.system_config.as_ref().map_or_else(
        || {
            let lines = vec![
                Line::from(""),
                Line::from(Span::styled(
                    "Loading system config...",
                    Style::default().fg(Color::DarkGray),
                )),
                Line::from(Span::styled("(Requires L1 RPC)", Style::default().fg(Color::DarkGray))),
            ];
            Paragraph::new(lines).alignment(Alignment::Center)
        },
        |sys| {
            let gas_limit_str = sys.gas_limit.to_string();
            let elasticity_str =
                sys.eip1559_elasticity.map(|e| e.to_string()).unwrap_or_else(|| "-".to_string());
            let denominator_str =
                sys.eip1559_denominator.map(|d| d.to_string()).unwrap_or_else(|| "-".to_string());

            let lines = vec![
                Line::from(vec![
                    Span::styled("Gas Limit: ", Style::default().fg(Color::DarkGray)),
                    Span::styled(gas_limit_str, Style::default().fg(Color::White)),
                ]),
                Line::from(vec![
                    Span::styled("EIP-1559 Elasticity: ", Style::default().fg(Color::DarkGray)),
                    Span::styled(elasticity_str, Style::default().fg(Color::White)),
                ]),
                Line::from(vec![
                    Span::styled("EIP-1559 Denominator: ", Style::default().fg(Color::DarkGray)),
                    Span::styled(denominator_str, Style::default().fg(Color::White)),
                ]),
            ];
            Paragraph::new(lines)
        },
    );

    f.render_widget(content.block(block), area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::MonitoringConfig;

    #[test]
    fn completed_refresh_updates_system_config() {
        let mut view = ConfigView::new();
        let mut resources = Resources::new(MonitoringConfig::mainnet());
        let expected = SystemConfig { gas_limit: 30_000_000, ..Default::default() };
        let (tx, rx) = oneshot::channel();
        view.refresh_rx = Some(rx);
        tx.send(Ok(expected)).expect("refresh result receiver should be open");

        assert_eq!(view.tick(&mut resources), Action::None);
        assert_eq!(resources.system_config, Some(expected));
        assert!(view.refresh_rx.is_none());
    }

    #[test]
    fn failed_refresh_preserves_last_valid_system_config() {
        let mut view = ConfigView::new();
        let mut resources = Resources::new(MonitoringConfig::mainnet());
        let existing = SystemConfig { gas_limit: 30_000_000, ..Default::default() };
        resources.system_config = Some(existing);
        let (tx, rx) = oneshot::channel();
        view.refresh_rx = Some(rx);
        tx.send(Err(anyhow::anyhow!("transient L1 failure")))
            .expect("refresh result receiver should be open");

        assert_eq!(view.tick(&mut resources), Action::None);
        assert_eq!(resources.system_config, Some(existing));
        assert!(view.refresh_rx.is_none());
    }
}
