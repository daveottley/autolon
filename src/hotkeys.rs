use crate::clicker::{Command, Status};
use anyhow::{Context, Result, anyhow, bail};
use futures_util::StreamExt;
use libc::{O_NONBLOCK, O_RDONLY, c_int, c_ulong, pollfd, timeval};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    mem,
    os::fd::AsRawFd,
    os::unix::fs::MetadataExt,
    os::unix::fs::OpenOptionsExt,
    path::PathBuf,
    sync::mpsc::{self, Sender},
    thread,
    time::{Duration, Instant},
};

const EV_SYN: u16 = 0x00;
const EV_KEY: u16 = 0x01;
const SYN_REPORT: u16 = 0;
const KEY_ENTER: u16 = 28;
const KEY_A: u16 = 30;
const KEY_SPACE: u16 = 57;
const KEY_F6: u16 = 64;
const KEY_F7: u16 = 65;
const KEY_MAX: u16 = 0x2ff;
const BUS_USB: u16 = 0x03;
const UINPUT_MAX_NAME_SIZE: usize = 80;
const KDE_COMPONENT: &str = "io.github.autolon.Autolon";
const KDE_COMPONENT_PATH: &str = "/component/io_github_autolon_Autolon";
const QT_KEY_F6: i32 = 16_777_269;
const QT_KEY_F7: i32 = 16_777_270;
const KDE_NO_AUTOLOADING: u32 = 0x4;

pub fn support_summary() -> String {
    match crate::config::Config::load_or_create() {
        Ok(config) if config.global_autoclicker_enabled => match readable_event_device_count() {
            Ok(count) if count > 0 => {
                "Global autoclicker enabled: direct F6/F7 override is ready".to_string()
            }
            Ok(_) if kde_global_shortcuts_available() => {
                "Global hotkeys enabled through KDE; direct Chrome override permission is missing"
                    .to_string()
            }
            Ok(_) if portal_global_shortcuts_version().is_ok() => {
                "Global hotkeys enabled through portal; direct Chrome override permission is missing"
                    .to_string()
            }
            Ok(_) => "Global hotkeys enabled, but input permission is missing".to_string(),
            Err(_) => "Global hotkeys enabled, but input status is unavailable".to_string(),
        },
        Ok(_) => {
            "Global autoclicker disabled: F6/F7 only affect the Autolon test canvas".to_string()
        }
        Err(_) => "Hotkey status unavailable".to_string(),
    }
}

pub fn readable_event_device_count() -> Result<usize> {
    Ok(open_keyboard_devices(false)?.len())
}

pub fn portal_global_shortcuts_version() -> Result<u32> {
    let connection = zbus::blocking::Connection::session()?;
    let proxy = zbus::blocking::Proxy::new(
        &connection,
        "org.freedesktop.portal.Desktop",
        "/org/freedesktop/portal/desktop",
        "org.freedesktop.portal.GlobalShortcuts",
    )?;
    proxy.get_property("version").map_err(Into::into)
}

pub fn kde_global_shortcuts_available() -> bool {
    zbus::blocking::Connection::session()
        .and_then(|connection| {
            let proxy = zbus::blocking::Proxy::new(
                &connection,
                "org.kde.kglobalaccel",
                "/kglobalaccel",
                "org.kde.KGlobalAccel",
            )?;
            proxy.call::<_, _, Vec<i32>>(
                "shortcut",
                &(vec![
                    KDE_COMPONENT.to_string(),
                    "cycle".to_string(),
                    "Autolon".to_string(),
                    "Cycle autoclicker".to_string(),
                ]),
            )
        })
        .map(|keys| keys.contains(&QT_KEY_F6))
        .unwrap_or(false)
}

pub fn direct_keyboard_device_count() -> Result<usize> {
    let mut count = 0;
    for path in event_paths()? {
        let Ok(file) = open_direct_event(&path) else {
            continue;
        };
        if looks_like_keyboard(file.as_raw_fd()) {
            count += 1;
        }
    }
    Ok(count)
}

pub fn logind_keyboard_device_count() -> Result<usize> {
    Ok(open_logind_keyboard_devices(false)?.len())
}

pub fn keyboard_candidates() -> Vec<String> {
    let Ok(text) = fs::read_to_string("/proc/bus/input/devices") else {
        return Vec::new();
    };
    let mut candidates = Vec::new();
    let mut name = String::new();
    let mut handlers = String::new();
    for line in text.lines().chain(std::iter::once("")) {
        if line.is_empty() {
            if handlers.contains("kbd") && handlers.contains("event") {
                candidates.push(format!("{name} {handlers}"));
            }
            name.clear();
            handlers.clear();
            continue;
        }
        if let Some(value) = line.strip_prefix("N: Name=") {
            name = value.to_string();
        } else if let Some(value) = line.strip_prefix("H: Handlers=") {
            handlers = value.to_string();
        }
    }
    candidates
}

pub fn spawn(tx: Sender<Command>) {
    thread::spawn(move || {
        loop {
            match crate::config::Config::load_or_create() {
                Ok(config) if config.global_autoclicker_enabled => {
                    if let Err(proxy_err) = run_keyboard_proxy(tx.clone()) {
                        eprintln!(
                            "autolon: keyboard-grab global hotkeys unavailable: {proxy_err:#}"
                        );
                        if let Err(kde_err) = run_kde_global_shortcuts(tx.clone()) {
                            eprintln!("autolon: KDE global shortcuts unavailable: {kde_err:#}");
                            if let Err(portal_err) = run_portal_shortcuts(tx.clone()) {
                                eprintln!(
                                    "autolon: portal global shortcuts unavailable: {portal_err:#}"
                                );
                            }
                        }
                        thread::sleep(Duration::from_secs(2));
                    }
                }
                Ok(_) => thread::sleep(Duration::from_millis(250)),
                Err(err) => {
                    eprintln!("autolon: hotkey config unavailable: {err:#}");
                    thread::sleep(Duration::from_secs(2));
                }
            }
        }
    });
}

fn run_kde_global_shortcuts(tx: Sender<Command>) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()?;
    runtime.block_on(run_kde_global_shortcuts_async(tx))
}

async fn run_kde_global_shortcuts_async(tx: Sender<Command>) -> Result<()> {
    let connection = zbus::Connection::session().await?;
    let accel = zbus::Proxy::new(
        &connection,
        "org.kde.kglobalaccel",
        "/kglobalaccel",
        "org.kde.KGlobalAccel",
    )
    .await?;
    register_kde_shortcut(&accel, "cycle", "Cycle autoclicker", QT_KEY_F6).await?;
    register_kde_shortcut(&accel, "emergency-stop", "Stop autoclicker", QT_KEY_F7).await?;
    accel
        .call::<_, _, ()>("activateGlobalShortcutContext", &(KDE_COMPONENT, "default"))
        .await?;

    let component = zbus::Proxy::new(
        &connection,
        "org.kde.kglobalaccel",
        KDE_COMPONENT_PATH,
        "org.kde.kglobalaccel.Component",
    )
    .await?;
    let mut pressed = component.receive_signal("globalShortcutPressed").await?;
    eprintln!("autolon: global F6/F7 registered through KDE global shortcuts");

    loop {
        if !crate::config::Config::load_or_create()
            .map(|config| config.global_autoclicker_enabled)
            .unwrap_or(false)
        {
            return Ok(());
        }

        match tokio::time::timeout(Duration::from_millis(250), pressed.next()).await {
            Ok(Some(message)) => {
                let (_component, shortcut, _timestamp): (String, String, i64) =
                    message.body().deserialize()?;
                match shortcut.as_str() {
                    "cycle" => dispatch(&tx, CommandKind::Cycle),
                    "emergency-stop" | "stop" => dispatch(&tx, CommandKind::Stop),
                    _ => {}
                }
            }
            Ok(None) => bail!("KDE global shortcut stream ended"),
            Err(_) => {}
        }
    }
}

async fn register_kde_shortcut(
    accel: &zbus::Proxy<'_>,
    shortcut_id: &str,
    description: &str,
    key: i32,
) -> Result<()> {
    let action_id = vec![
        KDE_COMPONENT.to_string(),
        shortcut_id.to_string(),
        "Autolon".to_string(),
        description.to_string(),
    ];
    accel
        .call::<_, _, ()>("doRegister", &(action_id.clone()))
        .await?;
    let _assigned: Vec<i32> = accel
        .call("setShortcut", &(action_id, vec![key], KDE_NO_AUTOLOADING))
        .await?;
    Ok(())
}

fn run_portal_shortcuts(tx: Sender<Command>) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()?;
    runtime.block_on(run_portal_shortcuts_async(tx))
}

async fn run_portal_shortcuts_async(tx: Sender<Command>) -> Result<()> {
    use ashpd::desktop::{
        CreateSessionOptions,
        global_shortcuts::{BindShortcutsOptions, GlobalShortcuts, NewShortcut},
    };

    let portal = GlobalShortcuts::new().await?;
    let mut activated = portal.receive_activated().await?;
    let session = portal
        .create_session(CreateSessionOptions::default())
        .await
        .context("failed to create global shortcuts portal session")?;
    let shortcuts = [
        NewShortcut::new("cycle", "Cycle Autoclick Speed").preferred_trigger(Some("F6")),
        NewShortcut::new("stop", "Emergency Stop").preferred_trigger(Some("F7")),
    ];
    let request = portal
        .bind_shortcuts(&session, &shortcuts, None, BindShortcutsOptions::default())
        .await
        .context("failed to bind F6/F7 through global shortcuts portal")?;
    let response = request.response()?;
    eprintln!(
        "autolon: global F6/F7 registered through desktop shortcuts ({} shortcut(s))",
        response.shortcuts().len()
    );

    loop {
        if !crate::config::Config::load_or_create()
            .map(|config| config.global_autoclicker_enabled)
            .unwrap_or(false)
        {
            let _ = session.close().await;
            return Ok(());
        }

        match tokio::time::timeout(Duration::from_millis(250), activated.next()).await {
            Ok(Some(event)) => match event.shortcut_id() {
                "cycle" => dispatch(&tx, CommandKind::Cycle),
                "stop" => dispatch(&tx, CommandKind::Stop),
                _ => {}
            },
            Ok(None) => bail!("global shortcuts portal activation stream ended"),
            Err(_) => {}
        }
    }
}

fn run_keyboard_proxy(tx: Sender<Command>) -> Result<()> {
    let mut devices = open_keyboard_devices(true)?;
    if devices.is_empty() {
        bail!(
            "no readable keyboard /dev/input/event* devices; install the packaged udev rule and reload udev"
        );
    }
    let mut keyboard = UinputKeyboard::open()?;
    eprintln!(
        "autolon: global F6/F7 proxy enabled through {} grabbed keyboard device(s)",
        devices.len()
    );

    let mut last_cycle = Instant::now() - Duration::from_secs(1);
    let mut last_stop = Instant::now() - Duration::from_secs(1);

    loop {
        if !crate::config::Config::load_or_create()
            .map(|config| config.global_autoclicker_enabled)
            .unwrap_or(false)
        {
            return Ok(());
        }

        let mut fds: Vec<pollfd> = devices
            .iter()
            .map(|device| pollfd {
                fd: device.file.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            })
            .collect();

        let ready = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, 250) };
        if ready < 0 {
            bail!(std::io::Error::last_os_error());
        }

        for (idx, fd) in fds.iter().enumerate() {
            if fd.revents & libc::POLLIN == 0 {
                continue;
            }
            while let Some(event) = devices[idx].read_event()? {
                match (event.type_, event.code, event.value) {
                    (EV_KEY, KEY_F6, 1) if last_cycle.elapsed() > Duration::from_millis(160) => {
                        last_cycle = Instant::now();
                        dispatch(&tx, CommandKind::Cycle);
                    }
                    (EV_KEY, KEY_F7, 1) if last_stop.elapsed() > Duration::from_millis(160) => {
                        last_stop = Instant::now();
                        dispatch(&tx, CommandKind::Stop);
                    }
                    (EV_KEY, KEY_F6 | KEY_F7, _) => {}
                    (EV_KEY, code, value) => keyboard.key(code, value)?,
                    (EV_SYN, SYN_REPORT, _) => keyboard.sync()?,
                    _ => {}
                }
            }
        }
    }
}

fn open_keyboard_devices(grab: bool) -> Result<Vec<EventDevice>> {
    let mut devices = Vec::new();
    for path in event_paths()? {
        let Ok(file) = open_direct_event(&path) else {
            continue;
        };
        if !looks_like_keyboard(file.as_raw_fd()) {
            continue;
        }
        let mut device = EventDevice {
            file,
            path,
            grabbed: false,
            logind: None,
        };
        if grab && let Err(err) = device.grab() {
            eprintln!("autolon: skipping keyboard device: {err:#}");
            continue;
        }
        devices.push(device);
    }
    if devices.is_empty()
        && let Ok(logind_devices) = open_logind_keyboard_devices(grab)
    {
        devices = logind_devices;
    }
    Ok(devices)
}

fn open_direct_event(path: &PathBuf) -> Result<File> {
    OpenOptions::new()
        .read(true)
        .custom_flags(O_RDONLY | O_NONBLOCK)
        .open(path)
        .map_err(Into::into)
}

fn event_paths() -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for entry in fs::read_dir("/dev/input").context("failed to read /dev/input")? {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name.starts_with("event") {
            paths.push(path);
        }
    }
    paths.sort();
    Ok(paths)
}

fn open_logind_keyboard_devices(grab: bool) -> Result<Vec<EventDevice>> {
    let session_id = std::env::var("XDG_SESSION_ID").context("XDG_SESSION_ID is not set")?;
    let connection = zbus::blocking::Connection::system()?;
    let reply = connection.call_method(
        Some("org.freedesktop.login1"),
        "/org/freedesktop/login1",
        Some("org.freedesktop.login1.Manager"),
        "GetSession",
        &(session_id.as_str()),
    )?;
    let session_path: zbus::zvariant::OwnedObjectPath = reply.body().deserialize()?;
    let session_path = session_path.to_string();

    let mut devices = Vec::new();
    for path in event_paths()? {
        let metadata = fs::metadata(&path)?;
        let (major, minor) = linux_dev_major_minor(metadata.rdev());
        let Ok((file, paused)) = take_logind_device(&connection, &session_path, major, minor)
        else {
            continue;
        };
        if paused || !looks_like_keyboard(file.as_raw_fd()) {
            let _ = release_logind_device(&connection, &session_path, major, minor);
            continue;
        }
        let mut device = EventDevice {
            file,
            path,
            grabbed: false,
            logind: Some(LogindLease {
                connection: connection.clone(),
                session_path: session_path.clone(),
                major,
                minor,
            }),
        };
        if grab && let Err(err) = device.grab() {
            eprintln!("autolon: skipping logind keyboard device: {err:#}");
            continue;
        }
        devices.push(device);
    }
    Ok(devices)
}

fn take_logind_device(
    connection: &zbus::blocking::Connection,
    session_path: &str,
    major: u32,
    minor: u32,
) -> Result<(File, bool)> {
    let reply = connection.call_method(
        Some("org.freedesktop.login1"),
        session_path,
        Some("org.freedesktop.login1.Session"),
        "TakeDevice",
        &(major, minor),
    )?;
    let (fd, paused): (zbus::zvariant::OwnedFd, bool) = reply.body().deserialize()?;
    Ok((File::from(std::os::fd::OwnedFd::from(fd)), paused))
}

fn release_logind_device(
    connection: &zbus::blocking::Connection,
    session_path: &str,
    major: u32,
    minor: u32,
) -> Result<()> {
    let _ = connection.call_method(
        Some("org.freedesktop.login1"),
        session_path,
        Some("org.freedesktop.login1.Session"),
        "ReleaseDevice",
        &(major, minor),
    )?;
    Ok(())
}

fn linux_dev_major_minor(dev: u64) -> (u32, u32) {
    let major = ((dev >> 8) & 0x0fff) | ((dev >> 32) & !0x0fff);
    let minor = (dev & 0x00ff) | ((dev >> 12) & !0x00ff);
    (major as u32, minor as u32)
}

fn looks_like_keyboard(fd: c_int) -> bool {
    has_key(fd, KEY_A) && has_key(fd, KEY_ENTER) && has_key(fd, KEY_SPACE) && has_key(fd, KEY_F6)
}

fn has_key(fd: c_int, key: u16) -> bool {
    let mut bits = [0_u8; 96];
    let rc = unsafe {
        libc::ioctl(
            fd,
            eviocgbit(EV_KEY as c_ulong, bits.len() as c_ulong),
            bits.as_mut_ptr(),
        )
    };
    if rc < 0 {
        return false;
    }
    let byte = key as usize / 8;
    let bit = key as usize % 8;
    bits.get(byte).is_some_and(|value| value & (1 << bit) != 0)
}

struct EventDevice {
    file: File,
    #[allow(dead_code)]
    path: PathBuf,
    grabbed: bool,
    logind: Option<LogindLease>,
}

struct LogindLease {
    connection: zbus::blocking::Connection,
    session_path: String,
    major: u32,
    minor: u32,
}

impl EventDevice {
    fn grab(&mut self) -> Result<()> {
        let rc = unsafe { libc::ioctl(self.file.as_raw_fd(), EVIOCGRAB, 1) };
        if rc < 0 {
            return Err(anyhow!(std::io::Error::last_os_error()))
                .context(format!("failed to grab {}", self.path.display()));
        }
        self.grabbed = true;
        Ok(())
    }

    fn read_event(&mut self) -> Result<Option<InputEvent>> {
        let mut bytes = [0_u8; mem::size_of::<InputEvent>()];
        match self.file.read_exact(&mut bytes) {
            Ok(()) => {
                let event =
                    unsafe { std::ptr::read_unaligned(bytes.as_ptr().cast::<InputEvent>()) };
                Ok(Some(event))
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
            Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => Ok(None),
            Err(err) => Err(err.into()),
        }
    }
}

impl Drop for EventDevice {
    fn drop(&mut self) {
        if self.grabbed {
            let _ = unsafe { libc::ioctl(self.file.as_raw_fd(), EVIOCGRAB, 0) };
        }
        if let Some(logind) = &self.logind {
            let _ = release_logind_device(
                &logind.connection,
                &logind.session_path,
                logind.major,
                logind.minor,
            );
        }
    }
}

struct UinputKeyboard {
    file: File,
}

impl UinputKeyboard {
    fn open() -> Result<Self> {
        let file = OpenOptions::new()
            .write(true)
            .custom_flags(O_NONBLOCK)
            .open("/dev/uinput")
            .context("failed to open /dev/uinput for virtual keyboard")?;

        unsafe {
            ioctl_none(file.as_raw_fd(), UI_SET_EVBIT, EV_KEY as c_ulong)?;
            ioctl_none(file.as_raw_fd(), UI_SET_EVBIT, EV_SYN as c_ulong)?;
            for code in 1..=KEY_MAX {
                ioctl_none(file.as_raw_fd(), UI_SET_KEYBIT, code as c_ulong)?;
            }
        }

        let mut setup: UinputSetup = unsafe { mem::zeroed() };
        setup.id.bustype = BUS_USB;
        setup.id.vendor = 0x1d6b;
        setup.id.product = 0x0107;
        let name = b"Autolon virtual keyboard\0";
        setup.name[..name.len()].copy_from_slice(name);

        unsafe {
            ui_dev_setup(file.as_raw_fd(), &setup)?;
            ioctl_none(file.as_raw_fd(), UI_DEV_CREATE, 0)?;
        }
        thread::sleep(Duration::from_millis(150));
        Ok(Self { file })
    }

    fn key(&mut self, code: u16, value: i32) -> Result<()> {
        self.event(EV_KEY, code, value)
    }

    fn sync(&mut self) -> Result<()> {
        self.event(EV_SYN, SYN_REPORT, 0)
    }

    fn event(&mut self, event_type: u16, code: u16, value: i32) -> Result<()> {
        let event = InputEvent {
            _time: timeval {
                tv_sec: 0,
                tv_usec: 0,
            },
            type_: event_type,
            code,
            value,
        };
        let bytes = unsafe {
            std::slice::from_raw_parts(
                (&event as *const InputEvent).cast::<u8>(),
                mem::size_of::<InputEvent>(),
            )
        };
        self.file.write_all(bytes)?;
        Ok(())
    }
}

impl Drop for UinputKeyboard {
    fn drop(&mut self) {
        let _ = unsafe { ioctl_none(self.file.as_raw_fd(), UI_DEV_DESTROY, 0) };
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct InputEvent {
    _time: timeval,
    type_: u16,
    code: u16,
    value: i32,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct InputId {
    bustype: u16,
    vendor: u16,
    product: u16,
    version: u16,
}

#[repr(C)]
struct UinputSetup {
    id: InputId,
    name: [u8; UINPUT_MAX_NAME_SIZE],
    ff_effects_max: u32,
}

enum CommandKind {
    Cycle,
    Stop,
}

fn dispatch(tx: &Sender<Command>, kind: CommandKind) {
    let (reply_tx, reply_rx) = mpsc::channel::<Result<Status, String>>();
    let command = match kind {
        CommandKind::Cycle => Command::Cycle(reply_tx),
        CommandKind::Stop => Command::Stop(reply_tx),
    };

    if tx.send(command).is_ok() {
        let _ = reply_rx.recv_timeout(Duration::from_secs(2));
    }
}

unsafe fn ui_dev_setup(fd: c_int, setup: &UinputSetup) -> Result<()> {
    let rc = unsafe { libc::ioctl(fd, UI_DEV_SETUP, setup as *const UinputSetup) };
    if rc < 0 {
        return Err(anyhow!(std::io::Error::last_os_error())).context("UI_DEV_SETUP failed");
    }
    Ok(())
}

unsafe fn ioctl_none(fd: c_int, request: c_ulong, arg: c_ulong) -> Result<()> {
    let rc = unsafe { libc::ioctl(fd, request, arg) };
    if rc < 0 {
        return Err(anyhow!(std::io::Error::last_os_error()));
    }
    Ok(())
}

const EVIOCGRAB: c_ulong = iow::<c_int>(b'E' as c_ulong, 0x90);
const UINPUT_IOCTL_BASE: c_ulong = b'U' as c_ulong;
const UI_DEV_CREATE: c_ulong = io(UINPUT_IOCTL_BASE, 1);
const UI_DEV_DESTROY: c_ulong = io(UINPUT_IOCTL_BASE, 2);
const UI_DEV_SETUP: c_ulong = iow::<UinputSetup>(UINPUT_IOCTL_BASE, 3);
const UI_SET_EVBIT: c_ulong = iow::<c_int>(UINPUT_IOCTL_BASE, 100);
const UI_SET_KEYBIT: c_ulong = iow::<c_int>(UINPUT_IOCTL_BASE, 101);

const IOC_NRBITS: c_ulong = 8;
const IOC_TYPEBITS: c_ulong = 8;
const IOC_SIZEBITS: c_ulong = 14;
const IOC_NRSHIFT: c_ulong = 0;
const IOC_TYPESHIFT: c_ulong = IOC_NRSHIFT + IOC_NRBITS;
const IOC_SIZESHIFT: c_ulong = IOC_TYPESHIFT + IOC_TYPEBITS;
const IOC_DIRSHIFT: c_ulong = IOC_SIZESHIFT + IOC_SIZEBITS;
const IOC_NONE: c_ulong = 0;
const IOC_WRITE: c_ulong = 1;
const IOC_READ: c_ulong = 2;

const fn ioc(dir: c_ulong, type_: c_ulong, nr: c_ulong, size: c_ulong) -> c_ulong {
    (dir << IOC_DIRSHIFT) | (type_ << IOC_TYPESHIFT) | (nr << IOC_NRSHIFT) | (size << IOC_SIZESHIFT)
}

const fn io(type_: c_ulong, nr: c_ulong) -> c_ulong {
    ioc(IOC_NONE, type_, nr, 0)
}

const fn iow<T>(type_: c_ulong, nr: c_ulong) -> c_ulong {
    ioc(IOC_WRITE, type_, nr, mem::size_of::<T>() as c_ulong)
}

const fn eviocgbit(ev: c_ulong, len: c_ulong) -> c_ulong {
    ioc(IOC_READ, b'E' as c_ulong, 0x20 + ev, len)
}
