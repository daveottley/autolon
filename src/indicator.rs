use crate::{
    clicker::{ClickerState, Command, Status},
    config::Config,
};
use anyhow::Result;
use serde::Serialize;
use std::{
    future,
    sync::{
        Arc, Mutex,
        mpsc::{self, Sender},
    },
    thread,
    time::Duration,
};
use zbus::{connection, interface};

pub const BUS_NAME: &str = "io.github.autolon.Autolon.Indicator";
pub const OBJECT_PATH: &str = "/io/github/autolon/Autolon/Indicator";

#[derive(Debug, Serialize)]
struct IndicatorState {
    running: bool,
    slot_id: u8,
    slot_name: String,
    interval_ms: u64,
    color: [f64; 3],
}

#[derive(Clone)]
struct IndicatorService {
    state_json: Arc<Mutex<String>>,
}

#[interface(name = "io.github.autolon.Autolon.Indicator")]
impl IndicatorService {
    #[zbus(name = "StateJson")]
    fn state_json(&self) -> String {
        self.state_json
            .lock()
            .map(|state| state.clone())
            .unwrap_or_else(|_| stopped_json())
    }
}

pub fn spawn(tx: Sender<Command>) {
    thread::spawn(move || {
        let state_json = Arc::new(Mutex::new(stopped_json()));
        seed_state(&tx, &state_json);
        subscribe_state(tx, state_json.clone());

        if let Err(err) = zbus::block_on(run(state_json)) {
            eprintln!("autolon: indicator service unavailable: {err:#}");
        }
    });
}

async fn run(state_json: Arc<Mutex<String>>) -> Result<()> {
    let _connection = connection::Builder::session()?
        .serve_at(OBJECT_PATH, IndicatorService { state_json })?
        .name(BUS_NAME)?
        .build()
        .await?;

    future::pending::<()>().await;
    Ok(())
}

fn seed_state(tx: &Sender<Command>, state_json: &Arc<Mutex<String>>) {
    let (reply_tx, reply_rx) = mpsc::channel();
    if tx.send(Command::Status(reply_tx)).is_ok()
        && let Ok(Ok(status)) = reply_rx.recv_timeout(Duration::from_secs(1))
    {
        update_state_json(state_json, status);
    }
}

fn subscribe_state(tx: Sender<Command>, state_json: Arc<Mutex<String>>) {
    let (status_tx, status_rx) = mpsc::channel();
    if tx.send(Command::SubscribeStatus(status_tx)).is_err() {
        return;
    }

    thread::spawn(move || {
        for status in status_rx {
            update_state_json(&state_json, status);
        }
    });
}

fn update_state_json(state_json: &Arc<Mutex<String>>, status: Status) {
    if let Ok(json) = indicator_state_json(status)
        && let Ok(mut state) = state_json.lock()
    {
        *state = json;
    }
}

fn indicator_state_json(status: Status) -> Result<String> {
    let state = match status.state {
        ClickerState::Running {
            slot_id,
            interval_ms,
            ..
        } => {
            let config = Config::load_or_create().unwrap_or_default();
            let slot_name = config
                .slot(slot_id)
                .map(|slot| slot.name.clone())
                .unwrap_or_else(|_| "User".to_string());
            IndicatorState {
                running: true,
                slot_id,
                slot_name: slot_name.clone(),
                interval_ms,
                color: active_slot_rgb(&slot_name),
            }
        }
        ClickerState::Stopped => IndicatorState {
            running: false,
            slot_id: 0,
            slot_name: String::new(),
            interval_ms: 0,
            color: [0.0, 0.0, 0.0],
        },
    };
    Ok(serde_json::to_string(&state)?)
}

fn stopped_json() -> String {
    serde_json::to_string(&IndicatorState {
        running: false,
        slot_id: 0,
        slot_name: String::new(),
        interval_ms: 0,
        color: [0.0, 0.0, 0.0],
    })
    .unwrap_or_else(|_| "{\"running\":false}".to_string())
}

fn active_slot_rgb(name: &str) -> [f64; 3] {
    match name {
        "Slow" => [0.10, 0.45, 0.95],
        "Fast" => [0.95, 0.55, 0.08],
        _ => [0.45, 0.32, 0.95],
    }
}
