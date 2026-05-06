use crate::config::{BackendPreference, Config, MouseButton};
use anyhow::{Context, Result, anyhow, bail};
use libc::{O_NONBLOCK, c_char, c_int, c_uint, c_ulong, c_void, timeval};
use std::{env, fs::OpenOptions, io::Write, mem, os::fd::AsRawFd, ptr, thread, time::Duration};

pub trait InputBackend: Send {
    fn name(&self) -> &'static str;
    fn click(&mut self, button: MouseButton, press_duration_ms: u64) -> Result<()>;
}

pub fn select_backend(config: &Config) -> Result<Box<dyn InputBackend>> {
    match config.backend {
        BackendPreference::Uinput => Ok(Box::new(UinputBackend::open()?)),
        BackendPreference::X11 => Ok(Box::new(X11Backend::open()?)),
        BackendPreference::Auto => {
            if is_wayland_session() {
                match UinputBackend::open() {
                    Ok(backend) => return Ok(Box::new(backend)),
                    Err(err) => {
                        if env::var_os("DISPLAY").is_some() {
                            eprintln!("autolon: uinput unavailable on Wayland path: {err:#}");
                        } else {
                            return Err(err).context("Wayland input requires uinput access in v0");
                        }
                    }
                }
            }
            if env::var_os("DISPLAY").is_some()
                && let Ok(backend) = X11Backend::open()
            {
                return Ok(Box::new(backend));
            }
            Ok(Box::new(UinputBackend::open()?))
        }
    }
}

pub fn is_wayland_session() -> bool {
    env::var_os("WAYLAND_DISPLAY").is_some()
        || env::var("XDG_SESSION_TYPE").is_ok_and(|value| value.eq_ignore_ascii_case("wayland"))
}

struct UinputBackend {
    file: std::fs::File,
}

impl UinputBackend {
    fn open() -> Result<Self> {
        let file = OpenOptions::new()
            .write(true)
            .custom_flags(O_NONBLOCK)
            .open("/dev/uinput")
            .context("failed to open /dev/uinput; add your user to the autolon-input group or run the packaged udev rule")?;

        unsafe {
            ui_set_evbit(file.as_raw_fd(), EV_KEY)?;
            ui_set_keybit(file.as_raw_fd(), BTN_LEFT)?;
            ui_set_keybit(file.as_raw_fd(), BTN_MIDDLE)?;
            ui_set_keybit(file.as_raw_fd(), BTN_RIGHT)?;
        }

        let mut setup: UinputSetup = unsafe { mem::zeroed() };
        setup.id.bustype = BUS_USB;
        setup.id.vendor = 0x1d6b;
        setup.id.product = 0x0104;
        let name = b"Autolon virtual pointer\0";
        setup.name[..name.len()].copy_from_slice(name);

        unsafe {
            ui_dev_setup(file.as_raw_fd(), &setup)?;
            ui_dev_create(file.as_raw_fd())?;
        }

        thread::sleep(Duration::from_millis(150));
        Ok(Self { file })
    }

    fn event(&mut self, event_type: u16, code: u16, value: i32) -> Result<()> {
        let event = InputEvent {
            time: timeval {
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

    fn sync(&mut self) -> Result<()> {
        self.event(EV_SYN, SYN_REPORT, 0)
    }
}

impl Drop for UinputBackend {
    fn drop(&mut self) {
        let _ = unsafe { ui_dev_destroy(self.file.as_raw_fd()) };
    }
}

impl InputBackend for UinputBackend {
    fn name(&self) -> &'static str {
        "uinput"
    }

    fn click(&mut self, button: MouseButton, press_duration_ms: u64) -> Result<()> {
        let code = match button {
            MouseButton::Left => BTN_LEFT,
            MouseButton::Middle => BTN_MIDDLE,
            MouseButton::Right => BTN_RIGHT,
        };
        self.event(EV_KEY, code, 1)?;
        self.sync()?;
        thread::sleep(Duration::from_millis(press_duration_ms));
        self.event(EV_KEY, code, 0)?;
        self.sync()
    }
}

struct X11Backend {
    display: *mut c_void,
}

unsafe impl Send for X11Backend {}

impl X11Backend {
    fn open() -> Result<Self> {
        let display = unsafe { XOpenDisplay(ptr::null()) };
        if display.is_null() {
            bail!("failed to open X11 display");
        }
        let mut event_base = 0;
        let mut error_base = 0;
        let ok = unsafe {
            XTestQueryExtension(display, &mut event_base, &mut error_base, &mut 0, &mut 0)
        };
        if ok == 0 {
            unsafe { XCloseDisplay(display) };
            bail!("XTEST extension is not available");
        }
        Ok(Self { display })
    }
}

impl Drop for X11Backend {
    fn drop(&mut self) {
        unsafe {
            XCloseDisplay(self.display);
        }
    }
}

impl InputBackend for X11Backend {
    fn name(&self) -> &'static str {
        "x11-xtest"
    }

    fn click(&mut self, button: MouseButton, press_duration_ms: u64) -> Result<()> {
        let button = match button {
            MouseButton::Left => 1,
            MouseButton::Middle => 2,
            MouseButton::Right => 3,
        };
        unsafe {
            XTestFakeButtonEvent(self.display, button, 1, 0);
            XFlush(self.display);
        }
        thread::sleep(Duration::from_millis(press_duration_ms));
        unsafe {
            XTestFakeButtonEvent(self.display, button, 0, 0);
            XFlush(self.display);
        }
        Ok(())
    }
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

#[repr(C)]
struct InputEvent {
    time: timeval,
    type_: u16,
    code: u16,
    value: i32,
}

trait OpenOptionsExt {
    fn custom_flags(&mut self, flags: c_int) -> &mut Self;
}

impl OpenOptionsExt for OpenOptions {
    fn custom_flags(&mut self, flags: c_int) -> &mut Self {
        std::os::unix::fs::OpenOptionsExt::custom_flags(self, flags)
    }
}

unsafe fn ui_set_evbit(fd: c_int, bit: u16) -> Result<()> {
    unsafe { ioctl_none(fd, UI_SET_EVBIT, bit as c_ulong) }
}

unsafe fn ui_set_keybit(fd: c_int, bit: u16) -> Result<()> {
    unsafe { ioctl_none(fd, UI_SET_KEYBIT, bit as c_ulong) }
}

unsafe fn ui_dev_setup(fd: c_int, setup: &UinputSetup) -> Result<()> {
    let rc = unsafe { libc::ioctl(fd, UI_DEV_SETUP, setup as *const UinputSetup) };
    if rc < 0 {
        return Err(anyhow!(std::io::Error::last_os_error())).context("UI_DEV_SETUP failed");
    }
    Ok(())
}

unsafe fn ui_dev_create(fd: c_int) -> Result<()> {
    unsafe { ioctl_none(fd, UI_DEV_CREATE, 0) }
}

unsafe fn ui_dev_destroy(fd: c_int) -> Result<()> {
    unsafe { ioctl_none(fd, UI_DEV_DESTROY, 0) }
}

unsafe fn ioctl_none(fd: c_int, request: c_ulong, arg: c_ulong) -> Result<()> {
    let rc = unsafe { libc::ioctl(fd, request, arg) };
    if rc < 0 {
        return Err(anyhow!(std::io::Error::last_os_error()));
    }
    Ok(())
}

const EV_SYN: u16 = 0x00;
const EV_KEY: u16 = 0x01;
const SYN_REPORT: u16 = 0;
const BTN_LEFT: u16 = 0x110;
const BTN_RIGHT: u16 = 0x111;
const BTN_MIDDLE: u16 = 0x112;
const BUS_USB: u16 = 0x03;
const UINPUT_MAX_NAME_SIZE: usize = 80;

const IOC_NRBITS: c_ulong = 8;
const IOC_TYPEBITS: c_ulong = 8;
const IOC_SIZEBITS: c_ulong = 14;
const IOC_NRSHIFT: c_ulong = 0;
const IOC_TYPESHIFT: c_ulong = IOC_NRSHIFT + IOC_NRBITS;
const IOC_SIZESHIFT: c_ulong = IOC_TYPESHIFT + IOC_TYPEBITS;
const IOC_DIRSHIFT: c_ulong = IOC_SIZESHIFT + IOC_SIZEBITS;
const IOC_NONE: c_ulong = 0;
const IOC_WRITE: c_ulong = 1;

const fn ioc(dir: c_ulong, type_: c_ulong, nr: c_ulong, size: c_ulong) -> c_ulong {
    (dir << IOC_DIRSHIFT) | (type_ << IOC_TYPESHIFT) | (nr << IOC_NRSHIFT) | (size << IOC_SIZESHIFT)
}

const fn io(type_: c_ulong, nr: c_ulong) -> c_ulong {
    ioc(IOC_NONE, type_, nr, 0)
}

const fn iow<T>(type_: c_ulong, nr: c_ulong) -> c_ulong {
    ioc(IOC_WRITE, type_, nr, mem::size_of::<T>() as c_ulong)
}

const UINPUT_IOCTL_BASE: c_ulong = b'U' as c_ulong;
const UI_DEV_CREATE: c_ulong = io(UINPUT_IOCTL_BASE, 1);
const UI_DEV_DESTROY: c_ulong = io(UINPUT_IOCTL_BASE, 2);
const UI_DEV_SETUP: c_ulong = iow::<UinputSetup>(UINPUT_IOCTL_BASE, 3);
const UI_SET_EVBIT: c_ulong = iow::<c_int>(UINPUT_IOCTL_BASE, 100);
const UI_SET_KEYBIT: c_ulong = iow::<c_int>(UINPUT_IOCTL_BASE, 101);

#[link(name = "X11")]
#[link(name = "Xtst")]
unsafe extern "C" {
    fn XOpenDisplay(display_name: *const c_char) -> *mut c_void;
    fn XCloseDisplay(display: *mut c_void) -> c_int;
    fn XFlush(display: *mut c_void) -> c_int;
    fn XTestQueryExtension(
        display: *mut c_void,
        event_base: *mut c_int,
        error_base: *mut c_int,
        major_version: *mut c_int,
        minor_version: *mut c_int,
    ) -> c_int;
    fn XTestFakeButtonEvent(
        display: *mut c_void,
        button: c_uint,
        is_press: c_int,
        delay: c_ulong,
    ) -> c_int;
}
