use crate::clicker::{Command, Status};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::PathBuf,
    process::{Command as ProcessCommand, Stdio},
    sync::mpsc::{self, Sender},
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
    SetSlotInterval { slot_id: u8, interval_ms: u64 },
    SetSlotEnabled { slot_id: u8, enabled: bool },
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Response {
    pub ok: bool,
    pub status: Option<Status>,
    pub error: Option<String>,
}

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
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let tx = tx.clone();
                thread::spawn(move || {
                    if let Err(err) = handle_stream(stream, tx) {
                        eprintln!("autolon: IPC error: {err:#}");
                    }
                });
            }
            Err(err) => eprintln!("autolon: connection failed: {err}"),
        }
    }
    Ok(())
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

fn handle_stream(mut stream: UnixStream, tx: Sender<Command>) -> Result<()> {
    let mut request = String::new();
    BufReader::new(stream.try_clone()?).read_line(&mut request)?;
    let request: Request = serde_json::from_str(&request)?;
    let response = dispatch(request, tx)?;
    let line = serde_json::to_string(&response)?;
    writeln!(stream, "{line}")?;
    Ok(())
}

fn dispatch(request: Request, tx: Sender<Command>) -> Result<Response> {
    let (reply_tx, reply_rx) = mpsc::channel();
    let command = match request {
        Request::Status => Command::Status(reply_tx),
        Request::Cycle => Command::Cycle(reply_tx),
        Request::Stop => Command::Stop(reply_tx),
        Request::Reload => Command::Reload(reply_tx),
        Request::Quit => Command::Quit(reply_tx),
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
