use crate::clicker::{Command, Status};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::PathBuf,
    process::{Command as ProcessCommand, Stdio},
    sync::{
        Arc, Mutex,
        mpsc::{self, Sender},
    },
    thread,
    time::Duration,
};

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "kebab-case")]
pub enum Request {
    Status,
    Cycle,
    Stop,
    Reload,
    Quit,
    SubscribeShutdown,
    SubscribeStatus,
    SetSlotInterval { slot_id: u8, interval_ms: u64 },
    SetSlotEnabled { slot_id: u8, enabled: bool },
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Response {
    pub ok: bool,
    pub status: Option<Status>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "kebab-case")]
pub enum Event {
    Shutdown,
    Status { status: Status },
}

type ShutdownSubscribers = Arc<Mutex<Vec<UnixStream>>>;

pub fn socket_path() -> Result<PathBuf> {
    if let Ok(runtime) = std::env::var("XDG_RUNTIME_DIR") {
        return Ok(PathBuf::from(runtime).join("autolon.sock"));
    }
    Ok(std::env::temp_dir().join(format!("autolon-{}.sock", unsafe { libc::geteuid() })))
}

pub fn serve(tx: Sender<Command>) -> Result<()> {
    let path = socket_path()?;
    if path.exists() {
        let _ = fs::remove_file(&path);
    }
    let listener =
        UnixListener::bind(&path).with_context(|| format!("failed to bind {}", path.display()))?;
    let subscribers = ShutdownSubscribers::default();
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let tx = tx.clone();
                let subscribers = subscribers.clone();
                thread::spawn(move || {
                    if let Err(err) = handle_stream(stream, tx, subscribers) {
                        eprintln!("autolon: IPC error: {err:#}");
                    }
                });
            }
            Err(err) => eprintln!("autolon: connection failed: {err}"),
        }
    }
    Ok(())
}

pub fn subscribe_shutdown() -> Result<()> {
    let path = socket_path()?;
    let mut stream = UnixStream::connect(&path)
        .with_context(|| format!("autolon daemon is not running at {}", path.display()))?;
    let line = serde_json::to_string(&Request::SubscribeShutdown)?;
    writeln!(stream, "{line}")?;

    let mut reader = BufReader::new(stream);
    let mut response = String::new();
    reader.read_line(&mut response)?;
    let response: Response = serde_json::from_str(&response)?;
    if !response.ok {
        bail!(
            "shutdown subscription rejected: {}",
            response
                .error
                .unwrap_or_else(|| "unknown error".to_string())
        );
    }

    let mut event = String::new();
    reader.read_line(&mut event)?;
    if event.is_empty() {
        bail!("shutdown subscription ended before an event was received");
    }
    match serde_json::from_str(&event)? {
        Event::Shutdown => Ok(()),
        Event::Status { .. } => bail!("received status event while waiting for shutdown"),
    }
}

pub fn subscribe_status(mut on_status: impl FnMut(Status) -> Result<()>) -> Result<()> {
    let path = socket_path()?;
    let mut stream = UnixStream::connect(&path)
        .with_context(|| format!("autolon daemon is not running at {}", path.display()))?;
    let line = serde_json::to_string(&Request::SubscribeStatus)?;
    writeln!(stream, "{line}")?;

    let mut reader = BufReader::new(stream);
    let mut response = String::new();
    reader.read_line(&mut response)?;
    let response: Response = serde_json::from_str(&response)?;
    if !response.ok {
        bail!(
            "status subscription rejected: {}",
            response
                .error
                .unwrap_or_else(|| "unknown error".to_string())
        );
    }

    loop {
        let mut event = String::new();
        reader.read_line(&mut event)?;
        if event.is_empty() {
            bail!("status subscription ended");
        }
        match serde_json::from_str(&event)? {
            Event::Status { status } => on_status(status)?,
            Event::Shutdown => return Ok(()),
        }
    }
}

pub fn send(request: Request) -> Result<Response> {
    let path = socket_path()?;
    let mut stream = UnixStream::connect(&path)
        .with_context(|| format!("autolon daemon is not running at {}", path.display()))?;
    stream.set_read_timeout(Some(Duration::from_secs(4)))?;
    let line = serde_json::to_string(&request)?;
    writeln!(stream, "{line}")?;
    let mut reader = BufReader::new(stream);
    let mut response = String::new();
    reader.read_line(&mut response)?;
    let response: Response = serde_json::from_str(&response)?;
    Ok(response)
}

pub fn ensure_daemon() -> Result<()> {
    if send(Request::Status).is_ok() {
        return Ok(());
    }

    let exe = std::env::current_exe().context("could not locate autolon executable")?;
    ProcessCommand::new(exe)
        .arg("daemon")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("failed to start autolon daemon")?;

    for _ in 0..40 {
        thread::sleep(Duration::from_millis(50));
        if send(Request::Status).is_ok() {
            return Ok(());
        }
    }

    bail!("autolon daemon did not become ready");
}

fn handle_stream(
    mut stream: UnixStream,
    tx: Sender<Command>,
    subscribers: ShutdownSubscribers,
) -> Result<()> {
    let mut request = String::new();
    BufReader::new(stream.try_clone()?).read_line(&mut request)?;
    let request: Request = serde_json::from_str(&request)?;
    if matches!(request, Request::SubscribeShutdown) {
        return subscribe_stream(stream, subscribers);
    }
    if matches!(request, Request::SubscribeStatus) {
        return subscribe_status_stream(stream, tx);
    }
    let is_quit = matches!(request, Request::Quit);
    let response = dispatch(request, tx)?;
    let line = serde_json::to_string(&response)?;
    writeln!(stream, "{line}")?;
    if is_quit && response.ok {
        notify_shutdown(&subscribers);
    }
    Ok(())
}

fn subscribe_stream(mut stream: UnixStream, subscribers: ShutdownSubscribers) -> Result<()> {
    let response = Response {
        ok: true,
        status: None,
        error: None,
    };
    let line = serde_json::to_string(&response)?;
    writeln!(stream, "{line}")?;
    subscribers
        .lock()
        .expect("shutdown subscriber list poisoned")
        .push(stream);
    Ok(())
}

fn subscribe_status_stream(mut stream: UnixStream, tx: Sender<Command>) -> Result<()> {
    let response = Response {
        ok: true,
        status: None,
        error: None,
    };
    let line = serde_json::to_string(&response)?;
    writeln!(stream, "{line}")?;

    let (status_tx, status_rx) = mpsc::channel();
    tx.send(Command::SubscribeStatus(status_tx))
        .context("daemon command loop is unavailable")?;
    for status in status_rx {
        let line = serde_json::to_string(&Event::Status { status })?;
        writeln!(stream, "{line}")?;
    }
    Ok(())
}

fn notify_shutdown(subscribers: &ShutdownSubscribers) {
    let Ok(line) = serde_json::to_string(&Event::Shutdown) else {
        return;
    };
    subscribers
        .lock()
        .expect("shutdown subscriber list poisoned")
        .retain_mut(|stream| writeln!(stream, "{line}").is_ok());
}

fn dispatch(request: Request, tx: Sender<Command>) -> Result<Response> {
    let (reply_tx, reply_rx) = mpsc::channel();
    let command = match request {
        Request::Status => Command::Status(reply_tx),
        Request::Cycle => Command::Cycle(reply_tx),
        Request::Stop => Command::Stop(reply_tx),
        Request::Reload => Command::Reload(reply_tx),
        Request::Quit => Command::Quit(reply_tx),
        Request::SubscribeShutdown => bail!("shutdown subscriptions are handled before dispatch"),
        Request::SubscribeStatus => bail!("status subscriptions are handled before dispatch"),
        Request::SetSlotInterval {
            slot_id,
            interval_ms,
        } => Command::SetSlotInterval {
            slot_id,
            interval_ms,
            reply: reply_tx,
        },
        Request::SetSlotEnabled { slot_id, enabled } => Command::SetSlotEnabled {
            slot_id,
            enabled,
            reply: reply_tx,
        },
    };
    tx.send(command)
        .context("daemon command loop is unavailable")?;
    match reply_rx.recv_timeout(Duration::from_secs(4)) {
        Ok(Ok(status)) => Ok(Response {
            ok: true,
            status: Some(status),
            error: None,
        }),
        Ok(Err(error)) => Ok(Response {
            ok: false,
            status: None,
            error: Some(error),
        }),
        Err(_) => bail!("daemon command timed out"),
    }
}
