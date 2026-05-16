use crate::{config::Config, input};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::{
    sync::mpsc::{self, Receiver, RecvTimeoutError, Sender},
    thread,
    time::{Duration, SystemTime},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum ClickerState {
    Stopped,
    Running {
        slot_id: u8,
        interval_ms: u64,
        started_at_unix_ms: u64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Status {
    pub state: ClickerState,
    pub backend: String,
    pub wayland: bool,
    pub hotkeys: String,
    pub config_path: String,
}

#[derive(Debug)]
pub enum Command {
    Cycle(Sender<Result<Status, String>>),
    Stop(Sender<Result<Status, String>>),
    SubscribeStatus(Sender<Status>),
    SetSlotInterval {
        slot_id: u8,
        interval_ms: u64,
        reply: Sender<Result<Status, String>>,
    },
    SetSlotEnabled {
        slot_id: u8,
        enabled: bool,
        reply: Sender<Result<Status, String>>,
    },
    Reload(Sender<Result<Status, String>>),
    Status(Sender<Result<Status, String>>),
    Quit(Sender<Result<Status, String>>),
}

pub fn start() -> Sender<Command> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || worker(rx));
    tx
}

fn worker(rx: Receiver<Command>) {
    let mut config = Config::load_or_create().unwrap_or_default();
    let config_path = crate::config::config_path()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|_| "unknown".to_string());
    let mut backend = match input::select_backend(&config) {
        Ok(backend) => backend,
        Err(err) => {
            eprintln!("autolon: no input backend available yet: {err:#}");
            Box::new(NoopBackend {
                reason: format!("{err:#}"),
            }) as Box<dyn input::InputBackend>
        }
    };
    let mut state = ClickerState::Stopped;
    let mut status_subscribers = Vec::new();
    let mut quit = false;

    while !quit {
        let timeout = match &state {
            ClickerState::Stopped => None,
            ClickerState::Running { interval_ms, .. } => Some(Duration::from_millis(*interval_ms)),
        };

        let message = match timeout {
            Some(timeout) => rx.recv_timeout(timeout),
            None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };

        match message {
            Ok(command) => {
                let reply = handle_command(
                    command,
                    &mut config,
                    &mut backend,
                    &mut state,
                    &config_path,
                    &mut quit,
                    &mut status_subscribers,
                );
                if let Err(err) = reply {
                    eprintln!("autolon: command failed: {err:#}");
                }
                broadcast_status(
                    &mut status_subscribers,
                    &state,
                    backend.name(),
                    &config_path,
                );
            }
            Err(RecvTimeoutError::Timeout) => {
                if let ClickerState::Running { slot_id, .. } = state
                    && let Ok(slot) = config.slot(slot_id)
                    && let Err(err) = backend.click(slot.button, slot.press_duration_ms)
                {
                    eprintln!("autolon: click failed: {err:#}");
                    state = ClickerState::Stopped;
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}

fn handle_command(
    command: Command,
    config: &mut Config,
    backend: &mut Box<dyn input::InputBackend>,
    state: &mut ClickerState,
    config_path: &str,
    quit: &mut bool,
    status_subscribers: &mut Vec<Sender<Status>>,
) -> Result<()> {
    match command {
        Command::Cycle(reply) => {
            *state = cycle_state(config, state);
            send(reply, status(state, backend.name(), config_path));
        }
        Command::Stop(reply) => {
            *state = ClickerState::Stopped;
            send(reply, status(state, backend.name(), config_path));
        }
        Command::SubscribeStatus(subscriber) => {
            status_subscribers.push(subscriber);
        }
        Command::SetSlotInterval {
            slot_id,
            interval_ms,
            reply,
        } => {
            let result = (|| {
                config.slot_mut(slot_id)?.interval_ms = interval_ms;
                config.save()?;
                status(state, backend.name(), config_path).map_err(anyhow::Error::msg)
            })()
            .map_err(|err: anyhow::Error| format!("{err:#}"));
            send(reply, result);
        }
        Command::SetSlotEnabled {
            slot_id,
            enabled,
            reply,
        } => {
            let result = (|| {
                config.slot_mut(slot_id)?.enabled = enabled;
                config.save()?;
                if matches!(state, ClickerState::Running { slot_id: running, .. } if *running == slot_id && !enabled) {
                    *state = ClickerState::Stopped;
                }
                status(state, backend.name(), config_path).map_err(anyhow::Error::msg)
            })()
            .map_err(|err: anyhow::Error| format!("{err:#}"));
            send(reply, result);
        }
        Command::Reload(reply) => {
            let result = (|| {
                *config = Config::load_or_create()?;
                *backend = input::select_backend(config)?;
                status(state, backend.name(), config_path).map_err(anyhow::Error::msg)
            })()
            .map_err(|err: anyhow::Error| format!("{err:#}"));
            send(reply, result);
        }
        Command::Status(reply) => {
            send(reply, status(state, backend.name(), config_path));
        }
        Command::Quit(reply) => {
            *state = ClickerState::Stopped;
            *quit = true;
            send(reply, status(state, backend.name(), config_path));
            thread::spawn(|| {
                thread::sleep(Duration::from_millis(100));
                std::process::exit(0);
            });
        }
    }
    Ok(())
}

pub fn cycle_state(config: &Config, state: &ClickerState) -> ClickerState {
    let enabled: Vec<u8> = config
        .clicker
        .cycle_order
        .iter()
        .copied()
        .filter(|slot_id| config.slot(*slot_id).is_ok_and(|slot| slot.enabled))
        .collect();

    if enabled.is_empty() {
        return ClickerState::Stopped;
    }

    let next_slot = match state {
        ClickerState::Stopped => enabled[0],
        ClickerState::Running { slot_id, .. } => {
            let Some(index) = enabled.iter().position(|id| id == slot_id) else {
                return ClickerState::Stopped;
            };
            if index + 1 >= enabled.len() {
                return ClickerState::Stopped;
            }
            enabled[index + 1]
        }
    };

    let interval_ms = config
        .slot(next_slot)
        .map(|slot| slot.interval_ms)
        .unwrap_or(config.clicker.min_interval_ms);
    ClickerState::Running {
        slot_id: next_slot,
        interval_ms,
        started_at_unix_ms: SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX),
    }
}

fn status(state: &ClickerState, backend: &str, config_path: &str) -> Result<Status, String> {
    Ok(Status {
        state: state.clone(),
        backend: backend.to_string(),
        wayland: input::is_wayland_session(),
        hotkeys: crate::hotkeys::support_summary(),
        config_path: config_path.to_string(),
    })
}

fn send(reply: Sender<Result<Status, String>>, status: Result<Status, String>) {
    let _ = reply.send(status);
}

fn broadcast_status(
    subscribers: &mut Vec<Sender<Status>>,
    state: &ClickerState,
    backend: &str,
    config_path: &str,
) {
    let Ok(status) = status(state, backend, config_path) else {
        return;
    };
    subscribers.retain(|subscriber| subscriber.send(status.clone()).is_ok());
}

struct NoopBackend {
    reason: String,
}

impl input::InputBackend for NoopBackend {
    fn name(&self) -> &'static str {
        "unavailable"
    }

    fn click(
        &mut self,
        _button: crate::config::MouseButton,
        _press_duration_ms: u64,
    ) -> Result<()> {
        anyhow::bail!("{}", self.reason)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycles_off_slow_fast_off_when_user_disabled() {
        let config = Config::default();
        let s0 = ClickerState::Stopped;
        let s1 = cycle_state(&config, &s0);
        assert!(matches!(s1, ClickerState::Running { slot_id: 1, .. }));
        assert_eq!(config.slot(1).unwrap().name, "Slow");
        let s2 = cycle_state(&config, &s1);
        assert!(matches!(s2, ClickerState::Running { slot_id: 2, .. }));
        assert_eq!(config.slot(2).unwrap().name, "Fast");
        let s3 = cycle_state(&config, &s2);
        assert!(matches!(s3, ClickerState::Stopped));
    }

    #[test]
    fn cycles_through_user_when_enabled() {
        let mut config = Config::default();
        config.slots.three.enabled = true;
        let s0 = ClickerState::Stopped;
        let s1 = cycle_state(&config, &s0);
        let s2 = cycle_state(&config, &s1);
        let s3 = cycle_state(&config, &s2);
        assert!(matches!(s3, ClickerState::Running { slot_id: 3, .. }));
    }

    #[test]
    fn disabled_slots_are_skipped() {
        let mut config = Config::default();
        config.slots.one.enabled = false;
        let s1 = cycle_state(&config, &ClickerState::Stopped);
        assert!(matches!(s1, ClickerState::Running { slot_id: 2, .. }));
        let s2 = cycle_state(&config, &s1);
        assert!(matches!(s2, ClickerState::Stopped));
    }
}
