use crate::{clicker, config::Config, desktop, gui, input, ipc};
use anyhow::{Context, Result, bail};
use clap::{Args as ClapArgs, Parser, Subcommand};
use std::{io::Write, thread};

const UDEV_RULE_PATH: &str = "/usr/lib/udev/rules.d/70-autolon-uinput.rules";
const UDEV_RULE: &str = concat!(
    "# Autolon needs to read/grab physical keyboard events and emit virtual mouse/keyboard\n",
    "# events for compositor-independent Wayland hotkeys and clicking.\n",
    "SUBSYSTEM==\"misc\", KERNEL==\"uinput\", TAG+=\"uaccess\", GROUP=\"autolon-input\", MODE=\"0660\", OPTIONS+=\"static_node=uinput\"\n",
    "SUBSYSTEM==\"input\", KERNEL==\"event*\", ENV{ID_INPUT_KEYBOARD}==\"1\", TAG+=\"uaccess\", GROUP=\"autolon-input\", MODE=\"0660\"\n",
);

#[derive(Debug, Parser)]
#[command(name = "autolon")]
#[command(version)]
#[command(about = "Native Linux autoclicker and local input automation controller")]
pub struct Args {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    Daemon,
    Gui,
    Status,
    DiagnoseHotkeys,
    Verify,
    Permissions {
        #[command(subcommand)]
        command: PermissionsCommand,
    },
    Cycle,
    Stop,
    Quit,
    TestClick {
        #[arg(long, value_parser = ["auto", "uinput", "x11"], default_value = "auto")]
        backend: String,
    },
    TestGlobalHotkey {
        #[arg(long, default_value_t = 20)]
        seconds: u64,
    },
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    Slot {
        #[command(subcommand)]
        command: SlotCommand,
    },
    Autostart {
        #[command(subcommand)]
        command: ToggleCommand,
    },
    DesktopIcon {
        #[command(subcommand)]
        command: DesktopIconCommand,
    },
    Launcher {
        #[command(subcommand)]
        command: ToggleCommand,
    },
    InstallDesktopFiles {
        #[arg(long, default_value = "/usr")]
        prefix: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    Print,
    Path,
}

#[derive(Debug, Subcommand)]
pub enum SlotCommand {
    Set(SlotSet),
}

#[derive(Debug, Subcommand)]
pub enum PermissionsCommand {
    Status,
    Install,
}

#[derive(Debug, ClapArgs)]
pub struct SlotSet {
    pub slot_id: u8,
    #[arg(long)]
    pub interval_ms: Option<u64>,
    #[arg(long)]
    pub enabled: Option<bool>,
}

#[derive(Debug, Subcommand)]
pub enum ToggleCommand {
    Enable,
    Disable,
}

#[derive(Debug, Subcommand)]
pub enum DesktopIconCommand {
    Install,
    Remove,
}

pub fn run(args: Args) -> Result<()> {
    match args.command.unwrap_or(Command::Gui) {
        Command::Daemon => run_daemon(),
        Command::Gui => {
            let _ = ipc::ensure_daemon();
            gui::run()
        }
        Command::Status => print_observed_status(),
        Command::DiagnoseHotkeys => diagnose_hotkeys(),
        Command::Verify => verify(),
        Command::Permissions { command } => match command {
            PermissionsCommand::Status => permissions_status(),
            PermissionsCommand::Install => install_permissions(),
        },
        Command::Cycle => print_status(call(ipc::Request::Cycle)?),
        Command::Stop => send_without_autostart(ipc::Request::Stop),
        Command::Quit => send_without_autostart(ipc::Request::Quit),
        Command::TestClick { backend } => test_click(&backend),
        Command::TestGlobalHotkey { seconds } => test_global_hotkey(seconds),
        Command::Config { command } => match command {
            ConfigCommand::Print => {
                println!("{}", toml::to_string_pretty(&Config::load_or_create()?)?);
                Ok(())
            }
            ConfigCommand::Path => {
                println!("{}", crate::config::config_path()?.display());
                Ok(())
            }
        },
        Command::Slot { command } => match command {
            SlotCommand::Set(set) => set_slot(set),
        },
        Command::Autostart { command } => match command {
            ToggleCommand::Enable => {
                desktop::set_autostart(true)?;
                println!("Autolon autostart enabled");
                Ok(())
            }
            ToggleCommand::Disable => {
                desktop::set_autostart(false)?;
                println!("Autolon autostart disabled");
                Ok(())
            }
        },
        Command::DesktopIcon { command } => match command {
            DesktopIconCommand::Install => {
                desktop::set_desktop_icon(true)?;
                println!("Autolon desktop icon installed");
                Ok(())
            }
            DesktopIconCommand::Remove => {
                desktop::set_desktop_icon(false)?;
                println!("Autolon desktop icon removed");
                Ok(())
            }
        },
        Command::Launcher { command } => match command {
            ToggleCommand::Enable => {
                desktop::install_user_files()?;
                println!("Autolon KDE launcher entry installed");
                Ok(())
            }
            ToggleCommand::Disable => {
                desktop::remove_user_files()?;
                println!("Autolon KDE launcher entry removed");
                Ok(())
            }
        },
        Command::InstallDesktopFiles { prefix } => {
            desktop::install_system_files(prefix)?;
            println!("Desktop integration files installed");
            Ok(())
        }
    }
}

fn run_daemon() -> Result<()> {
    if ipc::send(ipc::Request::Status).is_ok() {
        bail!("autolon daemon is already running");
    }
    Config::load_or_create()?;
    let _ = desktop::install_user_files();
    let tx = clicker::start();
    let ipc_tx = tx.clone();
    crate::hotkeys::spawn(tx.clone());
    crate::indicator::spawn(tx.clone());
    crate::tray::spawn();
    thread::spawn(move || {
        if let Err(err) = ipc::serve(ipc_tx) {
            eprintln!("autolon: IPC server exited: {err:#}");
        }
    });
    println!("Autolon daemon started");
    loop {
        thread::park();
    }
}

fn diagnose_hotkeys() -> Result<()> {
    let config = Config::load_or_create()?;
    println!(
        "global_autoclicker_enabled: {}",
        config.global_autoclicker_enabled
    );
    let candidates = crate::hotkeys::keyboard_candidates();
    println!("kernel_keyboard_candidates: {}", candidates.len());
    for candidate in candidates.iter().take(8) {
        println!("candidate: {candidate}");
    }
    match crate::hotkeys::direct_keyboard_device_count() {
        Ok(count) => println!("direct_readable_keyboard_devices: {count}"),
        Err(err) => println!("direct_readable_keyboard_devices_error: {err:#}"),
    }
    match crate::hotkeys::logind_keyboard_device_count() {
        Ok(count) => println!("logind_keyboard_devices: {count}"),
        Err(err) => println!("logind_keyboard_devices_error: {err:#}"),
    }
    match crate::hotkeys::readable_event_device_count() {
        Ok(count) => println!("usable_keyboard_event_devices: {count}"),
        Err(err) => println!("usable_keyboard_event_devices_error: {err:#}"),
    }
    match crate::hotkeys::portal_global_shortcuts_version() {
        Ok(version) => println!("portal_global_shortcuts_version: {version}"),
        Err(err) => println!("portal_global_shortcuts_error: {err:#}"),
    }
    println!(
        "kde_global_shortcuts_available: {}",
        crate::hotkeys::kde_global_shortcuts_available()
    );
    let uinput_ok = std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/uinput")
        .is_ok();
    println!("uinput_writable: {uinput_ok}");
    println!("groups: {}", current_groups());
    Ok(())
}

fn current_groups() -> String {
    std::process::Command::new("id")
        .arg("-nG")
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|groups| groups.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

fn permissions_status() -> Result<()> {
    let direct_count = crate::hotkeys::readable_event_device_count().unwrap_or(0);
    let uinput_ok = std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/uinput")
        .is_ok();
    let rule_installed = std::fs::read_to_string(UDEV_RULE_PATH)
        .map(|content| content.trim() == UDEV_RULE.trim())
        .unwrap_or(false);

    println!("direct_keyboard_grab_ready: {}", direct_count > 0);
    println!("usable_keyboard_event_devices: {direct_count}");
    println!("uinput_writable: {uinput_ok}");
    println!("udev_rule_installed: {rule_installed}");
    println!("udev_rule_path: {UDEV_RULE_PATH}");
    println!(
        "autolon_input_group_exists: {}",
        command_success("getent", &["group", "autolon-input"])
    );
    println!(
        "user_in_autolon_input_group: {}",
        current_groups()
            .split_whitespace()
            .any(|group| group == "autolon-input")
    );
    println!("groups: {}", current_groups());
    Ok(())
}

fn verify() -> Result<()> {
    let mut failures = 0_u32;
    let config = Config::load_or_create()?;

    check(
        "global autoclicker defaults enabled",
        config.global_autoclicker_enabled,
        &mut failures,
    );
    check(
        "cycle hotkey is F6",
        config.hotkeys.cycle.eq_ignore_ascii_case("F6"),
        &mut failures,
    );
    check(
        "emergency stop hotkey is F7",
        config.hotkeys.stop.eq_ignore_ascii_case("F7"),
        &mut failures,
    );
    check(
        "cycle order is Slow -> Fast -> User",
        config.clicker.cycle_order == [1, 2, 3],
        &mut failures,
    );
    verify_slot(&config, 1, "Slow", true, 500, &mut failures);
    verify_slot(&config, 2, "Fast", true, 10, &mut failures);
    verify_slot(&config, 3, "User", false, 1000, &mut failures);

    let uinput_ok = std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/uinput")
        .is_ok();
    check(
        "uinput click injection is writable",
        uinput_ok,
        &mut failures,
    );

    let direct_count = crate::hotkeys::readable_event_device_count().unwrap_or(0);
    check(
        "direct keyboard grab is ready for Chrome F6 override",
        direct_count > 0,
        &mut failures,
    );
    println!("INFO usable_keyboard_event_devices={direct_count}");
    println!(
        "INFO kde_global_shortcuts_available={}",
        crate::hotkeys::kde_global_shortcuts_available()
    );
    println!(
        "INFO portal_global_shortcuts_available={}",
        crate::hotkeys::portal_global_shortcuts_version().is_ok()
    );

    let was_running = ipc::send(ipc::Request::Status).is_ok();
    let daemon_status = call(ipc::Request::Status)?;
    let daemon_stopped = daemon_status
        .status
        .as_ref()
        .is_some_and(|status| matches!(status.state, clicker::ClickerState::Stopped));
    check(
        "daemon starts with clicker stopped",
        daemon_stopped,
        &mut failures,
    );
    if !was_running {
        let _ = ipc::send(ipc::Request::Quit);
    }

    if failures > 0 {
        bail!("autolon verification failed with {failures} failing check(s)");
    }
    println!("Autolon verification passed");
    Ok(())
}

fn verify_slot(
    config: &Config,
    slot_id: u8,
    name: &str,
    enabled: bool,
    interval_ms: u64,
    failures: &mut u32,
) {
    match config.slot(slot_id) {
        Ok(slot) => {
            check(
                &format!("slot {slot_id} name is {name}"),
                slot.name == name,
                failures,
            );
            check(
                &format!("slot {slot_id} enabled is {enabled}"),
                slot.enabled == enabled,
                failures,
            );
            check(
                &format!("slot {slot_id} interval is {interval_ms}ms"),
                slot.interval_ms == interval_ms,
                failures,
            );
        }
        Err(_) => check(&format!("slot {slot_id} exists"), false, failures),
    }
}

fn check(name: &str, passed: bool, failures: &mut u32) {
    if passed {
        println!("PASS {name}");
    } else {
        println!("FAIL {name}");
        *failures += 1;
    }
}

fn install_permissions() -> Result<()> {
    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let rule_path = runtime_dir.join(format!("autolon-uinput-{}.rules", std::process::id()));
    {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&rule_path)
            .with_context(|| format!("failed to create {}", rule_path.display()))?;
        file.write_all(UDEV_RULE.as_bytes())?;
    }

    println!("Requesting permission to install Autolon input access rule...");
    let status = std::process::Command::new("pkexec")
        .arg("/bin/sh")
        .arg("-c")
        .arg(format!(
            "/usr/bin/groupadd -r autolon-input 2>/dev/null || true; /usr/bin/install -Dm644 \"$1\" {UDEV_RULE_PATH} && /usr/bin/udevadm control --reload-rules && /usr/bin/udevadm trigger"
        ))
        .arg("autolon-permissions")
        .arg(&rule_path)
        .status()
        .context("failed to run pkexec; install the udev rule manually from README.md")?;
    let _ = std::fs::remove_file(&rule_path);
    if !status.success() {
        bail!("permission installation was cancelled or failed");
    }
    println!(
        "Autolon input permissions installed. Log out/in if direct_keyboard_grab_ready is still false."
    );
    permissions_status()
}

fn command_success(command: &str, args: &[&str]) -> bool {
    std::process::Command::new(command)
        .args(args)
        .status()
        .is_ok_and(|status| status.success())
}

fn set_slot(set: SlotSet) -> Result<()> {
    if !(1..=3).contains(&set.slot_id) {
        bail!("slot id must be 1, 2, or 3");
    }
    if let Some(interval_ms) = set.interval_ms {
        let response = call(ipc::Request::SetSlotInterval {
            slot_id: set.slot_id,
            interval_ms,
        })?;
        print_status(response)?;
    }
    if let Some(enabled) = set.enabled {
        let response = call(ipc::Request::SetSlotEnabled {
            slot_id: set.slot_id,
            enabled,
        })?;
        print_status(response)?;
    }
    if set.interval_ms.is_none() && set.enabled.is_none() {
        bail!("nothing to set; pass --interval-ms or --enabled");
    }
    Ok(())
}

fn call(request: ipc::Request) -> Result<ipc::Response> {
    if !matches!(request, ipc::Request::Quit) {
        let _ = ipc::ensure_daemon();
    }
    let response = ipc::send(request)?;
    if !response.ok {
        bail!(
            "{}",
            response
                .error
                .unwrap_or_else(|| "daemon command failed".to_string())
        );
    }
    Ok(response)
}

pub fn print_status(response: ipc::Response) -> Result<()> {
    let status = response.status.context("daemon returned no status")?;
    let state = match status.state {
        clicker::ClickerState::Stopped => "stopped".to_string(),
        clicker::ClickerState::Running {
            slot_id,
            interval_ms,
            ..
        } => format!("running slot {slot_id} ({interval_ms} ms)"),
    };
    println!("state: {state}");
    println!("backend: {}", status.backend);
    println!(
        "session: {}",
        if status.wayland {
            "wayland"
        } else {
            "x11/other"
        }
    );
    println!("hotkeys: {}", status.hotkeys);
    println!("config: {}", status.config_path);
    Ok(())
}

fn print_observed_status() -> Result<()> {
    match ipc::send(ipc::Request::Status) {
        Ok(response) => print_status(response),
        Err(_) => print_daemon_not_running(),
    }
}

fn send_without_autostart(request: ipc::Request) -> Result<()> {
    match ipc::send(request) {
        Ok(response) => print_status(response),
        Err(_) => print_daemon_not_running(),
    }
}

fn print_daemon_not_running() -> Result<()> {
    println!("state: daemon not running");
    println!("config: {}", crate::config::config_path()?.display());
    Ok(())
}

fn test_click(backend: &str) -> Result<()> {
    let mut config = Config::load_or_create()?;
    config.backend = match backend {
        "auto" => crate::config::BackendPreference::Auto,
        "uinput" => crate::config::BackendPreference::Uinput,
        "x11" => crate::config::BackendPreference::X11,
        _ => bail!("unknown backend"),
    };
    let mut backend = input::select_backend(&config)?;
    backend.click(crate::config::MouseButton::Left, 25)?;
    println!("test click sent through {}", backend.name());
    Ok(())
}

fn test_global_hotkey(seconds: u64) -> Result<()> {
    let config = Config::load_or_create()?;
    let slow = config.slot(1)?.clone();
    let was_running = ipc::send(ipc::Request::Status).is_ok();

    call(ipc::Request::Stop)?;
    call(ipc::Request::SetSlotEnabled {
        slot_id: 1,
        enabled: true,
    })?;
    call(ipc::Request::SetSlotInterval {
        slot_id: 1,
        interval_ms: 10_000,
    })?;

    println!("Global F6 test armed for {seconds}s.");
    println!(
        "Focus Chrome or another app and press physical F6 once. Do not press F7 during this test."
    );
    println!("The clicker interval is temporarily set to 10000ms so this should not emit a click.");

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(seconds);
    let mut detected = false;
    while std::time::Instant::now() < deadline {
        if let Ok(response) = ipc::send(ipc::Request::Status)
            && let Some(status) = response.status
            && matches!(status.state, clicker::ClickerState::Running { .. })
        {
            detected = true;
            break;
        }
        thread::sleep(std::time::Duration::from_millis(50));
    }

    let _ = ipc::send(ipc::Request::Stop);
    let _ = ipc::send(ipc::Request::SetSlotInterval {
        slot_id: 1,
        interval_ms: slow.interval_ms,
    });
    let _ = ipc::send(ipc::Request::SetSlotEnabled {
        slot_id: 1,
        enabled: slow.enabled,
    });
    if !was_running {
        let _ = ipc::send(ipc::Request::Quit);
    }

    if detected {
        println!("PASS physical F6 reached Autolon");
        Ok(())
    } else {
        bail!("physical F6 did not reach Autolon within {seconds}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::error::ErrorKind;

    #[test]
    fn exposes_version_flag() {
        let err = Args::try_parse_from(["autolon", "--version"]).unwrap_err();

        assert_eq!(err.kind(), ErrorKind::DisplayVersion);
        assert!(err.to_string().contains(env!("CARGO_PKG_VERSION")));
    }
}
