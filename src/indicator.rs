use crate::{
    clicker::{ClickerState, Command, Status},
    config::Config,
};
use anyhow::{Context, Result};
use serde::Serialize;
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, Region},
    delegate_compositor, delegate_layer, delegate_output, delegate_registry, delegate_shm,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    shell::{
        WaylandSurface,
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
    },
    shm::{
        Shm, ShmHandler,
        slot::{Buffer, SlotPool},
    },
};
use std::{
    fs, future,
    num::NonZeroU32,
    path::PathBuf,
    process::Command as ProcessCommand,
    sync::{
        Arc, Mutex,
        mpsc::{self, Sender},
    },
    thread,
    time::{Duration, SystemTime},
};
use wayland_client::{
    Connection, QueueHandle,
    globals::registry_queue_init,
    protocol::{wl_output, wl_shm, wl_surface},
};
use zbus::{connection, interface};

pub const BUS_NAME: &str = "io.github.autolon.Autolon.Indicator";
pub const OBJECT_PATH: &str = "/io/github/autolon/Autolon/Indicator";

const KWIN_BRIDGE_ID: &str = "autolon_indicator_cursor_bridge";
const DRAW_INTERVAL: Duration = Duration::from_millis(16);
const RING_RADIUS: f64 = 13.0;
const RING_STROKE: f64 = 2.0;

#[derive(Debug, Clone, Serialize)]
struct IndicatorState {
    running: bool,
    slot_id: u8,
    slot_name: String,
    interval_ms: u64,
    color: [f64; 3],
    overlay_enabled: bool,
    cursor_x: i32,
    cursor_y: i32,
    cursor_updated_unix_ms: u64,
}

impl Default for IndicatorState {
    fn default() -> Self {
        Self {
            running: false,
            slot_id: 0,
            slot_name: String::new(),
            interval_ms: 0,
            color: [0.0, 0.0, 0.0],
            overlay_enabled: false,
            cursor_x: 0,
            cursor_y: 0,
            cursor_updated_unix_ms: 0,
        }
    }
}

#[derive(Clone)]
struct IndicatorService {
    state: Arc<Mutex<IndicatorState>>,
}

#[interface(name = "io.github.autolon.Autolon.Indicator")]
impl IndicatorService {
    #[zbus(name = "StateJson")]
    fn state_json(&self) -> String {
        self.state
            .lock()
            .ok()
            .and_then(|state| serde_json::to_string(&*state).ok())
            .unwrap_or_else(|| stopped_json())
    }

    #[zbus(name = "UpdateCursor")]
    fn update_cursor(&self, x: i32, y: i32) {
        if let Ok(mut state) = self.state.lock() {
            state.cursor_x = x;
            state.cursor_y = y;
            state.cursor_updated_unix_ms = now_unix_ms();
        }
    }
}

pub fn spawn(tx: Sender<Command>) {
    thread::spawn(move || {
        let state = Arc::new(Mutex::new(IndicatorState::default()));
        let kwin_bridge_loaded = Arc::new(Mutex::new(false));

        seed_state(&tx, &state);
        spawn_dbus(state.clone());
        spawn_overlay(state.clone());

        if let Err(err) = wait_for_dbus_service() {
            eprintln!("autolon: indicator service unavailable before overlay setup: {err:#}");
        }
        reconcile_kwin_bridge(&state, &kwin_bridge_loaded);
        subscribe_state(tx, state, kwin_bridge_loaded);

        loop {
            thread::park();
        }
    });
}

fn spawn_dbus(state: Arc<Mutex<IndicatorState>>) {
    thread::spawn(move || {
        if let Err(err) = zbus::block_on(run_dbus(state)) {
            eprintln!("autolon: indicator service unavailable: {err:#}");
        }
    });
}

async fn run_dbus(state: Arc<Mutex<IndicatorState>>) -> Result<()> {
    let _connection = connection::Builder::session()?
        .serve_at(OBJECT_PATH, IndicatorService { state })?
        .name(BUS_NAME)?
        .build()
        .await?;

    future::pending::<()>().await;
    Ok(())
}

fn seed_state(tx: &Sender<Command>, state: &Arc<Mutex<IndicatorState>>) {
    let (reply_tx, reply_rx) = mpsc::channel();
    if tx.send(Command::Status(reply_tx)).is_ok()
        && let Ok(Ok(status)) = reply_rx.recv_timeout(Duration::from_secs(1))
    {
        update_state(state, status);
    }
}

fn subscribe_state(
    tx: Sender<Command>,
    state: Arc<Mutex<IndicatorState>>,
    kwin_bridge_loaded: Arc<Mutex<bool>>,
) {
    let (status_tx, status_rx) = mpsc::channel();
    if tx.send(Command::SubscribeStatus(status_tx)).is_err() {
        return;
    }

    thread::spawn(move || {
        for status in status_rx {
            update_state(&state, status);
            reconcile_kwin_bridge(&state, &kwin_bridge_loaded);
        }
    });
}

fn update_state(state: &Arc<Mutex<IndicatorState>>, status: Status) {
    let config = Config::load_or_create().unwrap_or_default();
    if let Ok(mut state_ref) = state.lock() {
        state_ref.overlay_enabled = config.display_global_mouse_overlay;
        match status.state {
            ClickerState::Running {
                slot_id,
                interval_ms,
                ..
            } => {
                let slot_name = config
                    .slot(slot_id)
                    .map(|slot| slot.name.clone())
                    .unwrap_or_else(|_| "User".to_string());
                state_ref.running = true;
                state_ref.slot_id = slot_id;
                state_ref.slot_name = slot_name.clone();
                state_ref.interval_ms = interval_ms;
                state_ref.color = active_slot_rgb(&slot_name);
            }
            ClickerState::Stopped => {
                state_ref.running = false;
                state_ref.slot_id = 0;
                state_ref.slot_name.clear();
                state_ref.interval_ms = 0;
                state_ref.color = [0.0, 0.0, 0.0];
            }
        }
    }
}

fn stopped_json() -> String {
    serde_json::to_string(&IndicatorState::default())
        .unwrap_or_else(|_| "{\"running\":false}".to_string())
}

fn reconcile_kwin_bridge(
    state: &Arc<Mutex<IndicatorState>>,
    kwin_bridge_loaded: &Arc<Mutex<bool>>,
) {
    let overlay_enabled = state
        .lock()
        .map(|state| state.overlay_enabled)
        .unwrap_or(false);
    let Ok(mut loaded) = kwin_bridge_loaded.lock() else {
        return;
    };

    if overlay_enabled && !*loaded {
        match load_kwin_cursor_bridge() {
            Ok(()) => *loaded = true,
            Err(err) => eprintln!("autolon: KDE cursor overlay bridge unavailable: {err:#}"),
        }
    } else if !overlay_enabled && *loaded {
        unload_kwin_cursor_bridge();
        *loaded = false;
    }
}

fn wait_for_dbus_service() -> Result<()> {
    let deadline = SystemTime::now() + Duration::from_secs(5);
    while SystemTime::now() < deadline {
        let output = ProcessCommand::new("qdbus6")
            .args([
                BUS_NAME,
                OBJECT_PATH,
                "io.github.autolon.Autolon.Indicator.StateJson",
            ])
            .output();
        if output.is_ok_and(|output| output.status.success()) {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(50));
    }

    anyhow::bail!("D-Bus service {BUS_NAME} was not ready")
}

fn load_kwin_cursor_bridge() -> Result<()> {
    unload_kwin_cursor_bridge();
    let path = kwin_bridge_script_path();
    fs::write(&path, KWIN_CURSOR_BRIDGE_SCRIPT)
        .with_context(|| format!("failed to write {}", path.display()))?;

    let output = ProcessCommand::new("qdbus6")
        .args([
            "org.kde.KWin",
            "/Scripting",
            "org.kde.kwin.Scripting.loadScript",
            &path.display().to_string(),
            KWIN_BRIDGE_ID,
        ])
        .output()
        .context("failed to load KWin cursor bridge")?;
    if !output.status.success() {
        anyhow::bail!(
            "qdbus6 loadScript failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let script_id = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<i32>()
        .context("KWin did not return a script id")?;
    if script_id <= 0 {
        anyhow::bail!("KWin returned invalid script id {script_id}");
    }

    wait_for_kwin_script(script_id)?;
    let output = ProcessCommand::new("qdbus6")
        .args(["org.kde.KWin", "/Scripting", "org.kde.kwin.Scripting.start"])
        .output()
        .context("failed to start KWin cursor bridge")?;
    if !output.status.success() {
        anyhow::bail!(
            "qdbus6 Scripting.start failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

fn wait_for_kwin_script(script_id: i32) -> Result<()> {
    let script_path = format!("/Scripting/Script{script_id}");
    let deadline = SystemTime::now() + Duration::from_secs(2);
    while SystemTime::now() < deadline {
        let output = ProcessCommand::new("qdbus6")
            .args([
                "org.kde.KWin",
                &script_path,
                "org.freedesktop.DBus.Peer.Ping",
            ])
            .output();
        if output.is_ok_and(|output| output.status.success()) {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(25));
    }

    anyhow::bail!("KWin script object {script_path} was not ready")
}

fn unload_kwin_cursor_bridge() {
    let _ = ProcessCommand::new("qdbus6")
        .args([
            "org.kde.KWin",
            "/Scripting",
            "org.kde.kwin.Scripting.unloadScript",
            KWIN_BRIDGE_ID,
        ])
        .output();
}

fn kwin_bridge_script_path() -> PathBuf {
    std::env::temp_dir().join("autolon-indicator-cursor-bridge.js")
}

const KWIN_CURSOR_BRIDGE_SCRIPT: &str = r#"print("autolon cursor overlay bridge start");
function publishCursor() {
    var cursor = workspace.cursorPos;
    callDBus("io.github.autolon.Autolon.Indicator", "/io/github/autolon/Autolon/Indicator", "io.github.autolon.Autolon.Indicator", "UpdateCursor", cursor.x, cursor.y);
}
workspace.cursorPosChanged.connect(publishCursor);
publishCursor();
"#;

fn spawn_overlay(state: Arc<Mutex<IndicatorState>>) {
    thread::spawn(move || {
        if let Err(err) = run_overlay(state) {
            eprintln!("autolon: global mouse overlay unavailable: {err:#}");
        }
    });
}

fn run_overlay(state: Arc<Mutex<IndicatorState>>) -> Result<()> {
    let conn = Connection::connect_to_env().context("could not connect to Wayland compositor")?;
    let (globals, mut event_queue) = registry_queue_init(&conn)?;
    let qh = event_queue.handle();

    let compositor = CompositorState::bind(&globals, &qh)?;
    let layer_shell = LayerShell::bind(&globals, &qh).context("wlr layer-shell unavailable")?;
    let shm = Shm::bind(&globals, &qh)?;
    let surface = compositor.create_surface(&qh);
    let empty_input = Region::new(&compositor)?;
    surface.set_input_region(Some(empty_input.wl_region()));

    let layer = layer_shell.create_layer_surface(
        &qh,
        surface,
        Layer::Overlay,
        Some("autolon-cursor-overlay"),
        None,
    );
    layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
    layer.set_size(0, 0);
    layer.set_exclusive_zone(-1);
    layer.set_keyboard_interactivity(KeyboardInteractivity::None);
    layer.commit();
    let pool = SlotPool::new(4, &shm)?;

    let mut app = OverlayApp {
        registry_state: RegistryState::new(&globals),
        output_state: OutputState::new(&globals, &qh),
        shm,
        state,
        _empty_input: empty_input,
        layer,
        pool,
        buffer: None,
        width: 1,
        height: 1,
        configured: false,
        exit: false,
    };

    while !app.configured && !app.exit {
        event_queue.blocking_dispatch(&mut app)?;
    }

    while !app.exit {
        event_queue.dispatch_pending(&mut app)?;
        app.draw();
        if app.exit {
            break;
        }
        conn.flush()?;
        event_queue.roundtrip(&mut app)?;
        thread::sleep(DRAW_INTERVAL);
    }

    Ok(())
}

struct OverlayApp {
    registry_state: RegistryState,
    output_state: OutputState,
    shm: Shm,
    state: Arc<Mutex<IndicatorState>>,
    _empty_input: Region,
    layer: LayerSurface,
    pool: SlotPool,
    buffer: Option<Buffer>,
    width: u32,
    height: u32,
    configured: bool,
    exit: bool,
}

impl OverlayApp {
    fn draw(&mut self) {
        if !self.configured {
            return;
        }

        let width = self.width.max(1);
        let height = self.height.max(1);
        let stride = width as i32 * 4;
        let state = self.drawable_state();

        if self
            .buffer
            .as_ref()
            .is_some_and(|buffer| buffer.height() != height as i32 || buffer.stride() != stride)
        {
            self.buffer = None;
        }

        let buffer = self.buffer.get_or_insert_with(|| {
            self.pool
                .create_buffer(
                    width as i32,
                    height as i32,
                    stride,
                    wl_shm::Format::Argb8888,
                )
                .expect("create overlay buffer")
                .0
        });

        let canvas = match self.pool.canvas(buffer) {
            Some(canvas) => canvas,
            None => {
                let Ok((second_buffer, canvas)) = self.pool.create_buffer(
                    width as i32,
                    height as i32,
                    stride,
                    wl_shm::Format::Argb8888,
                ) else {
                    return;
                };
                *buffer = second_buffer;
                canvas
            }
        };

        canvas.fill(0);
        if let Some(state) = state {
            draw_indicator(canvas, width, height, &state);
        }

        self.layer
            .wl_surface()
            .damage_buffer(0, 0, width as i32, height as i32);
        let _ = buffer.attach_to(self.layer.wl_surface());
        self.layer.commit();
    }

    fn drawable_state(&self) -> Option<IndicatorState> {
        let state = self.state.lock().ok()?.clone();
        (state.overlay_enabled && state.running && state.cursor_updated_unix_ms != 0)
            .then_some(state)
    }
}

impl CompositorHandler for OverlayApp {
    fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _new_factor: i32,
    ) {
    }

    fn transform_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _new_transform: wl_output::Transform,
    ) {
    }

    fn frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _time: u32,
    ) {
    }

    fn surface_enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for OverlayApp {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
    }

    fn update_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
    }

    fn output_destroyed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
    }
}

impl LayerShellHandler for OverlayApp {
    fn closed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _layer: &LayerSurface) {
        self.exit = true;
    }

    fn configure(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        self.width = NonZeroU32::new(configure.new_size.0).map_or(1, NonZeroU32::get);
        self.height = NonZeroU32::new(configure.new_size.1).map_or(1, NonZeroU32::get);
        self.configured = true;
    }
}

impl ShmHandler for OverlayApp {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

fn draw_indicator(canvas: &mut [u8], width: u32, height: u32, state: &IndicatorState) {
    let x = (state.cursor_x as f64).clamp(RING_RADIUS, width as f64 - RING_RADIUS - 1.0);
    let y = (state.cursor_y as f64).clamp(RING_RADIUS, height as f64 - RING_RADIUS - 1.0);
    let color = rgb(state.color);
    draw_ring(
        canvas,
        width,
        height,
        x,
        y,
        RING_RADIUS,
        RING_STROKE,
        color,
        184,
    );
    draw_text(
        canvas,
        width,
        height,
        &state.interval_ms.to_string(),
        (x + 16.0).round() as i32,
        (y - 4.0).round() as i32,
        color,
        220,
    );
}

fn draw_ring(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    cx: f64,
    cy: f64,
    radius: f64,
    stroke: f64,
    color: [u8; 3],
    alpha: u8,
) {
    let min_x = (cx - radius - stroke - 1.0).floor().max(0.0) as i32;
    let max_x = (cx + radius + stroke + 1.0).ceil().min(width as f64 - 1.0) as i32;
    let min_y = (cy - radius - stroke - 1.0).floor().max(0.0) as i32;
    let max_y = (cy + radius + stroke + 1.0).ceil().min(height as f64 - 1.0) as i32;
    let half = stroke / 2.0;

    for py in min_y..=max_y {
        for px in min_x..=max_x {
            let dx = px as f64 + 0.5 - cx;
            let dy = py as f64 + 0.5 - cy;
            let distance = (dx * dx + dy * dy).sqrt();
            let coverage = (half + 0.7 - (distance - radius).abs()).clamp(0.0, 1.0);
            if coverage > 0.0 {
                put_pixel(
                    canvas,
                    width,
                    px,
                    py,
                    color,
                    (alpha as f64 * coverage).round() as u8,
                );
            }
        }
    }
}

fn draw_text(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    text: &str,
    x: i32,
    y: i32,
    color: [u8; 3],
    alpha: u8,
) {
    let mut cursor = x;
    for ch in text.chars() {
        if let Some(glyph) = glyph(ch) {
            draw_glyph(canvas, width, height, glyph, cursor, y, color, alpha);
            cursor += 8;
        }
    }
}

fn draw_glyph(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    glyph: [&str; 5],
    x: i32,
    y: i32,
    color: [u8; 3],
    alpha: u8,
) {
    for (row, bits) in glyph.iter().enumerate() {
        for (col, bit) in bits.as_bytes().iter().enumerate() {
            if *bit == b'1' {
                let px = x + col as i32 * 2;
                let py = y + row as i32 * 2;
                for dy in 0..2 {
                    for dx in 0..2 {
                        if px + dx >= 0
                            && py + dy >= 0
                            && px + dx < width as i32
                            && py + dy < height as i32
                        {
                            put_pixel(canvas, width, px + dx, py + dy, color, alpha);
                        }
                    }
                }
            }
        }
    }
}

fn put_pixel(canvas: &mut [u8], width: u32, x: i32, y: i32, color: [u8; 3], alpha: u8) {
    let offset = ((y as u32 * width + x as u32) * 4) as usize;
    if offset + 3 >= canvas.len() {
        return;
    }

    let a = alpha as u32;
    let r = color[0] as u32 * a / 255;
    let g = color[1] as u32 * a / 255;
    let b = color[2] as u32 * a / 255;
    let argb = (a << 24) | (r << 16) | (g << 8) | b;
    canvas[offset..offset + 4].copy_from_slice(&argb.to_le_bytes());
}

fn glyph(ch: char) -> Option<[&'static str; 5]> {
    match ch {
        '0' => Some(["111", "101", "101", "101", "111"]),
        '1' => Some(["010", "110", "010", "010", "111"]),
        '2' => Some(["111", "001", "111", "100", "111"]),
        '3' => Some(["111", "001", "111", "001", "111"]),
        '4' => Some(["101", "101", "111", "001", "001"]),
        '5' => Some(["111", "100", "111", "001", "111"]),
        '6' => Some(["111", "100", "111", "101", "111"]),
        '7' => Some(["111", "001", "010", "010", "010"]),
        '8' => Some(["111", "101", "111", "101", "111"]),
        '9' => Some(["111", "101", "111", "001", "111"]),
        _ => None,
    }
}

fn rgb(color: [f64; 3]) -> [u8; 3] {
    [
        (color[0].clamp(0.0, 1.0) * 255.0).round() as u8,
        (color[1].clamp(0.0, 1.0) * 255.0).round() as u8,
        (color[2].clamp(0.0, 1.0) * 255.0).round() as u8,
    ]
}

fn active_slot_rgb(name: &str) -> [f64; 3] {
    match name {
        "Slow" => [0.10, 0.45, 0.95],
        "Fast" => [0.95, 0.55, 0.08],
        _ => [0.45, 0.32, 0.95],
    }
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

delegate_compositor!(OverlayApp);
delegate_output!(OverlayApp);
delegate_shm!(OverlayApp);
delegate_layer!(OverlayApp);
delegate_registry!(OverlayApp);

impl ProvidesRegistryState for OverlayApp {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }

    registry_handlers![OutputState];
}
