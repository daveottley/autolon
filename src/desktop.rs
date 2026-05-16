use crate::config::APP_ID;
use anyhow::{Context, Result};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
};

const DESKTOP_FILE: &str = r#"[Desktop Entry]
Type=Application
Name=Autolon
GenericName=Input Automation Utility
Comment=Local autoclicker and input automation controller
Exec=autolon gui
Icon=io.github.autolon.Autolon
Terminal=false
Categories=Utility;
StartupNotify=false
StartupWMClass=io.github.autolon.Autolon
"#;

const AUTOSTART_FILE: &str = r#"[Desktop Entry]
Type=Application
Name=Autolon
GenericName=Input Automation Utility
Comment=Start Autolon daemon at login
Exec=autolon daemon
Icon=io.github.autolon.Autolon
Terminal=false
Categories=Utility;Game;
StartupNotify=false
StartupWMClass=io.github.autolon.Autolon
X-GNOME-Autostart-enabled=true
"#;

const ICON: &str = r##"<?xml version="1.0" encoding="utf-8"?>
<!-- Based on SVG Repo "Mouse Pointer Click" icon, CC0 License:
     https://www.svgrepo.com/svg/389320/mouse-pointer-click -->
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 128 128">
  <rect width="128" height="128" rx="24" fill="#111827"/>
  <g fill="none" stroke-linecap="round" stroke-linejoin="round">
    <path d="M47 42l28 67 10-30 30-10-68-27z" fill="#f8fafc" stroke="#020617" stroke-width="8"/>
    <path d="M85 80l24 24" stroke="#f8fafc" stroke-width="10"/>
    <path d="M85 80l24 24" stroke="#2563eb" stroke-width="5"/>
    <path d="M32 15l6 22M21 47l-22-6M69 20L53 36M37 69L21 85" stroke="#f59e0b" stroke-width="9"/>
  </g>
</svg>
"##;

const METINFO: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<component type="desktop-application">
  <id>io.github.autolon.Autolon</id>
  <name>Autolon</name>
  <summary>Local autoclicker and input automation controller</summary>
  <description>
    <p>Autolon provides a native Linux daemon, CLI, tray menu, and settings window for transparent local autoclicker control.</p>
  </description>
  <url type="homepage">https://github.com/autolon/autolon</url>
  <metadata_license>MIT</metadata_license>
  <project_license>MIT</project_license>
  <launchable type="desktop-id">io.github.autolon.Autolon.desktop</launchable>
</component>
"#;

const SYSUSERS: &str = "g autolon-input - - -\n";

const KWIN_INDICATOR_ID: &str = "autolonindicator";

const KWIN_INDICATOR_METADATA: &str = r#"{
    "KPackageStructure": "KWin/Script",
    "KPlugin": {
        "Authors": [
            {
                "Name": "Autolon"
            }
        ],
        "Category": "Accessibility",
        "Description": "Shows Autolon's active autoclick speed at the global cursor",
        "EnabledByDefault": false,
        "Id": "autolonindicator",
        "License": "MIT",
        "Name": "Autolon Cursor Indicator"
    },
    "X-Plasma-API": "declarativescript",
    "X-Plasma-MainScript": "code/main.qml"
}
"#;

const KWIN_INDICATOR_QML: &str = r##"import QtQuick
import QtQuick.Window
import org.kde.kwin

Window {
    id: root
    property bool indicatorRunning: false
    property int intervalMs: 0
    property var color: [0.45, 0.32, 0.95]

    x: Workspace.virtualScreenGeometry.x
    y: Workspace.virtualScreenGeometry.y
    width: Workspace.virtualScreenGeometry.width
    height: Workspace.virtualScreenGeometry.height
    color: "transparent"
    visible: indicatorRunning
    flags: Qt.X11BypassWindowManagerHint | Qt.WindowTransparentForInput

    function refreshState() {
        stateCall.call();
    }

    DBusCall {
        id: stateCall
        service: "io.github.autolon.Autolon.Indicator"
        path: "/io/github/autolon/Autolon/Indicator"
        dbusInterface: "io.github.autolon.Autolon.Indicator"
        method: "StateJson"
        onFinished: function(returnValue) {
            try {
                    var state = JSON.parse(returnValue[0]);
                    root.indicatorRunning = state.running === true;
                    root.intervalMs = state.interval_ms || 0;
                    root.color = state.color || [0.45, 0.32, 0.95];
                } catch (error) {
                    root.indicatorRunning = false;
                }
            }
            onFailed: function() {
            root.indicatorRunning = false;
        }
    }

    Timer {
        interval: 40
        repeat: true
        running: true
        onTriggered: root.refreshState()
    }

    Canvas {
        id: canvas
        anchors.fill: parent
        onPaint: {
            var ctx = getContext("2d");
            ctx.clearRect(0, 0, width, height);
            if (!root.indicatorRunning) {
                return;
            }

            var cursor = Workspace.cursorPos;
            var screenGeometry = Workspace.virtualScreenGeometry;
            var x = cursor.x - screenGeometry.x;
            var y = cursor.y - screenGeometry.y;
            if (x < -24 || y < -24 || x > width + 24 || y > height + 24) {
                return;
            }

            var radius = 13;
            x = Math.max(radius, Math.min(width - radius, x));
            y = Math.max(radius, Math.min(height - radius, y));
            var red = root.color[0];
            var green = root.color[1];
            var blue = root.color[2];
            var stroke = "rgba(" + Math.round(red * 255) + "," + Math.round(green * 255) + "," + Math.round(blue * 255) + ",0.72)";
            var text = "rgba(" + Math.round(red * 255) + "," + Math.round(green * 255) + "," + Math.round(blue * 255) + ",0.86)";

            ctx.lineWidth = 2;
            ctx.strokeStyle = stroke;
            ctx.beginPath();
            ctx.arc(x, y, radius, 0, Math.PI * 2, false);
            ctx.stroke();

            ctx.fillStyle = text;
            ctx.font = "bold 10px sans-serif";
            ctx.fillText(String(root.intervalMs), x + 16, y + 4);
        }

        Timer {
            interval: 16
            repeat: true
            running: root.indicatorRunning
            onTriggered: canvas.requestPaint()
        }
    }
}
"##;

pub fn set_autostart(enable: bool) -> Result<()> {
    let path = dirs::config_dir()
        .context("could not determine XDG config directory")?
        .join("autostart")
        .join(format!("{APP_ID}.desktop"));
    if enable {
        write_file(&path, &with_current_exec(AUTOSTART_FILE)?, 0o644)
    } else {
        remove_if_exists(&path)
    }
}

pub fn set_desktop_icon(enable: bool) -> Result<()> {
    let path = desktop_dir().join(format!("{APP_ID}.desktop"));
    if enable {
        write_file(&path, &with_current_exec(DESKTOP_FILE)?, 0o755)
    } else {
        remove_if_exists(&path)
    }
}

pub fn install_user_files() -> Result<()> {
    let data = dirs::data_dir().context("could not determine XDG data directory")?;
    let icon_root = data.join("icons/hicolor");
    write_file(
        &data.join("applications").join(format!("{APP_ID}.desktop")),
        &with_current_exec(DESKTOP_FILE)?,
        0o644,
    )?;
    write_file(
        &icon_root
            .join("scalable/apps")
            .join(format!("{APP_ID}.svg")),
        ICON,
        0o644,
    )?;
    install_raster_icons(&icon_root)?;
    install_kwin_indicator(&data)?;
    refresh_icon_cache(&icon_root);
    Ok(())
}

pub fn remove_user_files() -> Result<()> {
    if let Some(data) = dirs::data_dir() {
        remove_if_exists(&data.join("applications").join(format!("{APP_ID}.desktop")))?;
        remove_if_exists(
            &data
                .join("icons/hicolor/scalable/apps")
                .join(format!("{APP_ID}.svg")),
        )?;
        let _ = unload_legacy_kwin_effect();
        let _ = unload_kwin_indicator();
        remove_dir_if_exists(&data.join("kwin/effects").join(KWIN_INDICATOR_ID))?;
        remove_dir_if_exists(&data.join("kwin/scripts").join(KWIN_INDICATOR_ID))?;
    }
    Ok(())
}

pub fn install_system_files(prefix: String) -> Result<()> {
    let prefix = PathBuf::from(prefix);
    write_file(
        &prefix
            .join("share/applications")
            .join(format!("{APP_ID}.desktop")),
        DESKTOP_FILE,
        0o644,
    )?;
    write_file(
        &prefix
            .join("share/icons/hicolor/scalable/apps")
            .join(format!("{APP_ID}.svg")),
        ICON,
        0o644,
    )?;
    write_file(
        &prefix
            .join("share/metainfo")
            .join(format!("{APP_ID}.metainfo.xml")),
        METINFO,
        0o644,
    )?;
    write_file(
        &prefix.join("lib/sysusers.d").join("autolon.conf"),
        SYSUSERS,
        0o644,
    )?;
    write_file(
        &prefix
            .join("share/kwin/scripts")
            .join(KWIN_INDICATOR_ID)
            .join("metadata.json"),
        KWIN_INDICATOR_METADATA,
        0o644,
    )?;
    write_file(
        &prefix
            .join("share/kwin/scripts")
            .join(KWIN_INDICATOR_ID)
            .join("contents/code/main.qml"),
        KWIN_INDICATOR_QML,
        0o644,
    )?;
    Ok(())
}

pub fn current_exe_command() -> Result<String> {
    let exe = std::env::current_exe().context("could not locate autolon executable")?;
    Ok(shell_quote(&exe.display().to_string()))
}

fn desktop_dir() -> PathBuf {
    dirs::desktop_dir()
        .or_else(|| dirs::home_dir().map(|home| home.join("Desktop")))
        .unwrap_or_else(|| PathBuf::from("Desktop"))
}

fn write_file(path: &Path, text: &str, mode: u32) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, text).with_context(|| format!("failed to write {}", path.display()))?;
    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(mode);
    fs::set_permissions(path, permissions)?;
    Ok(())
}

fn install_raster_icons(icon_root: &Path) -> Result<()> {
    let svg_path = icon_root
        .join("scalable/apps")
        .join(format!("{APP_ID}.svg"));
    for size in [32, 48, 64, 128, 256] {
        let png_path = icon_root
            .join(format!("{size}x{size}/apps"))
            .join(format!("{APP_ID}.png"));
        if let Some(parent) = png_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let status = Command::new("rsvg-convert")
            .arg("-w")
            .arg(size.to_string())
            .arg("-h")
            .arg(size.to_string())
            .arg("-o")
            .arg(&png_path)
            .arg(&svg_path)
            .status();
        if !matches!(status, Ok(status) if status.success()) {
            break;
        }
    }
    Ok(())
}

fn refresh_icon_cache(icon_root: &Path) {
    let _ = Command::new("gtk-update-icon-cache")
        .arg("-q")
        .arg("-t")
        .arg("-f")
        .arg(icon_root)
        .status();
}

fn install_kwin_indicator(data: &Path) -> Result<()> {
    unload_legacy_kwin_effect()?;
    remove_dir_if_exists(&data.join("kwin/effects").join(KWIN_INDICATOR_ID))?;

    let script_root = data.join("kwin/scripts").join(KWIN_INDICATOR_ID);
    write_file(
        &script_root.join("metadata.json"),
        KWIN_INDICATOR_METADATA,
        0o644,
    )?;
    write_file(
        &script_root.join("contents/code/main.qml"),
        KWIN_INDICATOR_QML,
        0o644,
    )?;
    load_kwin_indicator(&script_root);
    Ok(())
}

fn load_kwin_indicator(script_root: &Path) {
    set_kwin_script_enabled(true);
    reconfigure_kwin();
    let _ = Command::new("qdbus6")
        .args([
            "org.kde.KWin",
            "/Scripting",
            "org.kde.kwin.Scripting.unloadScript",
            KWIN_INDICATOR_ID,
        ])
        .status();
    let _ = Command::new("qdbus6")
        .args([
            "org.kde.KWin",
            "/Scripting",
            "org.kde.kwin.Scripting.loadDeclarativeScript",
            &script_root
                .join("contents/code/main.qml")
                .display()
                .to_string(),
            KWIN_INDICATOR_ID,
        ])
        .status();
    let _ = Command::new("qdbus6")
        .args(["org.kde.KWin", "/Scripting", "org.kde.kwin.Scripting.start"])
        .status();
}

fn unload_kwin_indicator() -> Result<()> {
    set_kwin_script_enabled(false);
    let _ = Command::new("qdbus6")
        .args([
            "org.kde.KWin",
            "/Scripting",
            "org.kde.kwin.Scripting.unloadScript",
            KWIN_INDICATOR_ID,
        ])
        .status();
    reconfigure_kwin();
    Ok(())
}

fn unload_legacy_kwin_effect() -> Result<()> {
    set_kwin_effect_enabled(false);
    let _ = Command::new("qdbus6")
        .args([
            "org.kde.KWin",
            "/Effects",
            "org.kde.kwin.Effects.unloadEffect",
            KWIN_INDICATOR_ID,
        ])
        .status();
    reconfigure_kwin();
    Ok(())
}

fn set_kwin_script_enabled(enabled: bool) {
    let key = format!("{KWIN_INDICATOR_ID}Enabled");
    let value = if enabled { "true" } else { "false" };
    let _ = Command::new("kwriteconfig6")
        .args([
            "--file", "kwinrc", "--group", "Plugins", "--key", &key, value,
        ])
        .status();
}

fn set_kwin_effect_enabled(enabled: bool) {
    let value = if enabled { "true" } else { "false" };
    for key in [
        format!("{KWIN_INDICATOR_ID}Enabled"),
        format!("kwin4_effect_{KWIN_INDICATOR_ID}Enabled"),
    ] {
        let _ = Command::new("kwriteconfig6")
            .args([
                "--file", "kwinrc", "--group", "Plugins", "--key", &key, value,
            ])
            .status();
    }
}

fn reconfigure_kwin() {
    let _ = Command::new("qdbus6")
        .args(["org.kde.KWin", "/KWin", "org.kde.KWin.reconfigure"])
        .status();
}

fn with_current_exec(template: &str) -> Result<String> {
    let exe = current_exe_command()?;
    Ok(template
        .replace("Exec=autolon gui", &format!("Exec={exe} gui"))
        .replace("Exec=autolon daemon", &format!("Exec={exe} daemon")))
}

fn shell_quote(value: &str) -> String {
    if value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._-".contains(c))
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn remove_if_exists(path: &Path) -> Result<()> {
    if path.exists() {
        fs::remove_file(path).with_context(|| format!("failed to remove {}", path.display()))?;
    }
    Ok(())
}

fn remove_dir_if_exists(path: &Path) -> Result<()> {
    if path.exists() {
        fs::remove_dir_all(path).with_context(|| format!("failed to remove {}", path.display()))?;
    }
    Ok(())
}
