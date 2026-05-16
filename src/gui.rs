use crate::{config::Config, ipc};
use anyhow::Result;
use gtk4::gdk;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{
    Application, ApplicationWindow, Box as GtkBox, Button, CheckButton, DrawingArea,
    EventControllerKey, EventControllerMotion, GestureClick, Label, Orientation, SpinButton,
};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

const CANVAS_WIDTH: i32 = 680;
const CANVAS_HEIGHT: i32 = 360;
const MAX_PIXELS: usize = 60_000;
const DOT_SIZE: f64 = 5.0;
const DOT_LIFETIME: Duration = Duration::from_secs(4);
const ACTIVE_RING_RADIUS: f64 = 13.0;

#[derive(Clone, Copy)]
struct Dot {
    x: f64,
    y: f64,
    created: Instant,
    color: DotColor,
}

#[derive(Clone, Copy)]
enum DotColor {
    Red,
    Yellow,
    Blue,
}

impl DotColor {
    fn rgb(self) -> (f64, f64, f64) {
        match self {
            DotColor::Red => (0.95, 0.10, 0.12),
            DotColor::Yellow => (1.0, 0.82, 0.0),
            DotColor::Blue => (0.10, 0.32, 0.95),
        }
    }
}

pub fn run() -> Result<()> {
    let _ = crate::desktop::install_user_files();
    let app = Application::builder()
        .application_id(crate::config::APP_ID)
        .build();
    app.connect_activate(build_ui);
    app.run_with_args(&["autolon"]);
    Ok(())
}

#[derive(Clone)]
struct UiState {
    config: Config,
    dots: Vec<Dot>,
    next_dot_color: usize,
    pointer: (f64, f64),
    local_slot: Option<u8>,
    last_click: Instant,
}

fn build_ui(app: &Application) {
    let config = Config::load_or_create().unwrap_or_default();
    let state = Rc::new(RefCell::new(UiState {
        config,
        dots: Vec::new(),
        next_dot_color: 0,
        pointer: (CANVAS_WIDTH as f64 / 2.0, CANVAS_HEIGHT as f64 / 2.0),
        local_slot: None,
        last_click: Instant::now(),
    }));

    let root = GtkBox::new(Orientation::Vertical, 14);
    root.set_margin_top(16);
    root.set_margin_bottom(16);
    root.set_margin_start(16);
    root.set_margin_end(16);

    let status = Label::new(Some(&initial_status(&state.borrow().config)));
    status.set_xalign(0.0);
    root.append(&status);

    let hotkeys_title = Label::new(None);
    hotkeys_title.set_markup("<b><big>Hotkeys</big></b>");
    hotkeys_title.set_xalign(0.0);
    root.append(&hotkeys_title);

    let hotkeys = Label::new(Some("Cycle Autoclick Speed: F6\nEmergency Stop: F7"));
    hotkeys.set_xalign(0.0);
    root.append(&hotkeys);

    let enable_global = Button::with_label(if state.borrow().config.global_autoclicker_enabled {
        "Disable Global Autoclicker"
    } else {
        "Enable Global Autoclicker"
    });
    root.append(&enable_global);

    for slot_id in [1_u8, 2, 3] {
        append_slot(&root, state.clone(), status.clone(), slot_id);
    }

    let canvas = DrawingArea::builder()
        .width_request(CANVAS_WIDTH)
        .height_request(CANVAS_HEIGHT)
        .focusable(true)
        .build();
    canvas.set_hexpand(true);
    canvas.set_vexpand(true);
    canvas.set_draw_func({
        let state = state.clone();
        move |_, cr, width, height| {
            cr.set_source_rgb(1.0, 1.0, 1.0);
            let _ = cr.paint();
            cr.set_source_rgb(0.82, 0.85, 0.90);
            cr.rectangle(0.5, 0.5, width as f64 - 1.0, height as f64 - 1.0);
            let _ = cr.stroke();
            let now = Instant::now();
            for dot in &state.borrow().dots {
                let age = now.saturating_duration_since(dot.created);
                if age >= DOT_LIFETIME {
                    continue;
                }
                let alpha = 1.0 - (age.as_secs_f64() / DOT_LIFETIME.as_secs_f64());
                let (red, green, blue) = dot.color.rgb();
                cr.set_source_rgba(red, green, blue, alpha);
                cr.rectangle(dot.x, dot.y, DOT_SIZE, DOT_SIZE);
                let _ = cr.fill();
            }
            let state_ref = state.borrow();
            if let Some(slot_id) = state_ref.local_slot
                && let Ok(slot) = state_ref.config.slot(slot_id)
            {
                let (red, green, blue) = active_slot_rgb(&slot.name);
                let (x, y) = state_ref.pointer;
                cr.set_line_width(2.0);
                cr.set_source_rgba(red, green, blue, 0.72);
                cr.arc(
                    x.clamp(ACTIVE_RING_RADIUS, width as f64 - ACTIVE_RING_RADIUS),
                    y.clamp(ACTIVE_RING_RADIUS, height as f64 - ACTIVE_RING_RADIUS),
                    ACTIVE_RING_RADIUS,
                    0.0,
                    std::f64::consts::TAU,
                );
                let _ = cr.stroke();
            }
        }
    });

    let motion = EventControllerMotion::new();
    motion.connect_motion({
        let state = state.clone();
        move |_, x, y| {
            state.borrow_mut().pointer = (x, y);
        }
    });
    canvas.add_controller(motion);

    let click = GestureClick::new();
    click.connect_pressed({
        let state = state.clone();
        let canvas = canvas.clone();
        move |gesture, _, x, y| {
            if let Some(widget) = gesture.widget() {
                let _ = widget.grab_focus();
            }
            add_pixel(&state, x, y);
            canvas.queue_draw();
        }
    });
    canvas.add_controller(click);

    let capture_click = GestureClick::new();
    capture_click.set_propagation_phase(gtk4::PropagationPhase::Capture);
    capture_click.connect_pressed({
        let state = state.clone();
        let canvas = canvas.clone();
        move |gesture, _, _, _| {
            if state.borrow().local_slot.is_some() {
                let _ = canvas.grab_focus();
                gesture.set_state(gtk4::EventSequenceState::Claimed);
            }
        }
    });
    root.add_controller(capture_click);

    root.append(&canvas);

    let actions = GtkBox::new(Orientation::Horizontal, 8);
    let quit = Button::with_label("Quit Autoclicker");
    actions.append(&quit);
    root.append(&actions);

    enable_global.connect_clicked({
        let state = state.clone();
        let status = status.clone();
        let enable_global = enable_global.clone();
        let canvas = canvas.clone();
        move |_| {
            let mut state_ref = state.borrow_mut();
            let new_enabled = !state_ref.config.global_autoclicker_enabled;
            state_ref.config.global_autoclicker_enabled = new_enabled;
            match state_ref.config.save() {
                Ok(()) => {
                    let _ = ipc::send(ipc::Request::Reload);
                    if new_enabled {
                        enable_global.set_label("Disable Global Autoclicker");
                        match crate::hotkeys::readable_event_device_count() {
                            Ok(count) if count > 0 => {
                                status.set_label("Global autoclicker hotkeys enabled")
                            }
                            Ok(_) if crate::hotkeys::kde_global_shortcuts_available() => status
                                .set_label(
                                    "Global hotkeys enabled; Chrome override needs permissions",
                                ),
                            Ok(_) if crate::hotkeys::portal_global_shortcuts_version().is_ok() => {
                                status.set_label(
                                    "Global hotkeys enabled; Chrome override needs permissions",
                                )
                            }
                            Ok(_) => status
                                .set_label("Global hotkeys enabled, but permission is missing"),
                            Err(err) => status.set_label(&format!(
                                "Global hotkeys enabled, but input access failed: {err:#}"
                            )),
                        }
                    } else {
                        enable_global.set_label("Enable Global Autoclicker");
                        status.set_label("Global autoclicker hotkeys disabled");
                    }
                }
                Err(err) => status.set_label(&format!("Global toggle failed: {err:#}")),
            }
            canvas.grab_focus();
        }
    });

    let app_for_quit = app.clone();
    quit.connect_clicked(move |_| {
        let _ = ipc::send(ipc::Request::Quit);
        app_for_quit.quit();
    });

    glib::timeout_add_local(Duration::from_millis(1), {
        let state = state.clone();
        let canvas = canvas.clone();
        move || {
            let mut state_ref = state.borrow_mut();
            let before_retain = state_ref.dots.len();
            let now = Instant::now();
            state_ref
                .dots
                .retain(|dot| now.saturating_duration_since(dot.created) < DOT_LIFETIME);
            let mut needs_draw =
                before_retain != state_ref.dots.len() || !state_ref.dots.is_empty();
            if let Some(slot_id) = state_ref.local_slot
                && let Ok(slot) = state_ref.config.slot(slot_id)
                && state_ref.last_click.elapsed() >= Duration::from_millis(slot.interval_ms)
            {
                let (x, y) = state_ref.pointer;
                push_pixel(&mut state_ref, x, y);
                state_ref.last_click = Instant::now();
                needs_draw = true;
            }
            if needs_draw {
                canvas.queue_draw();
            }
            glib::ControlFlow::Continue
        }
    });

    let window = ApplicationWindow::builder()
        .application(app)
        .title("Autolon")
        .default_width(760)
        .default_height(700)
        .child(&root)
        .build();
    let key = EventControllerKey::new();
    key.connect_key_pressed({
        let state = state.clone();
        let status = status.clone();
        let canvas = canvas.clone();
        move |_, key, _, _| match key {
            gdk::Key::F6 => {
                cycle_local(&state, &status);
                canvas.queue_draw();
                glib::Propagation::Stop
            }
            gdk::Key::F7 => {
                state.borrow_mut().local_slot = None;
                status.set_label("Stopped");
                canvas.queue_draw();
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        }
    });
    window.add_controller(key);
    window.connect_is_active_notify({
        let state = state.clone();
        let status = status.clone();
        let canvas = canvas.clone();
        move |window| {
            if !window.is_active() && state.borrow().local_slot.is_some() {
                state.borrow_mut().local_slot = None;
                status.set_label("Stopped");
                canvas.queue_draw();
            }
        }
    });
    window.set_icon_name(Some(crate::config::APP_ID));
    window.present();
    canvas.grab_focus();
}

fn append_slot(root: &GtkBox, state: Rc<RefCell<UiState>>, status: Label, slot_id: u8) {
    let slot = state
        .borrow()
        .config
        .slot(slot_id)
        .cloned()
        .unwrap_or_default();

    let row = GtkBox::new(Orientation::Horizontal, 10);
    let enabled = CheckButton::with_label(&slot.name);
    enabled.set_active(slot.enabled);
    let speed = SpinButton::with_range(
        state.borrow().config.clicker.min_interval_ms as f64,
        10_000.0,
        1.0,
    );
    speed.set_value(slot.interval_ms as f64);
    let ms = Label::new(Some("ms"));
    row.append(&enabled);
    row.append(&speed);
    row.append(&ms);
    root.append(&row);

    enabled.connect_toggled({
        let state = state.clone();
        let status = status.clone();
        move |enabled| {
            let mut state_ref = state.borrow_mut();
            if let Ok(slot) = state_ref.config.slot_mut(slot_id) {
                slot.enabled = enabled.is_active();
            }
            save_ui_config(&state_ref.config, &status);
        }
    });

    speed.connect_value_changed(move |speed| {
        let mut state_ref = state.borrow_mut();
        if let Ok(slot) = state_ref.config.slot_mut(slot_id) {
            slot.interval_ms = speed.value() as u64;
            if slot.press_duration_ms >= slot.interval_ms {
                slot.press_duration_ms = slot.interval_ms.saturating_sub(1).max(1);
            }
        }
        save_ui_config(&state_ref.config, &status);
    });
}

fn initial_status(config: &Config) -> String {
    if !config.global_autoclicker_enabled {
        return "Global autoclicker hotkeys disabled".to_string();
    }
    match crate::hotkeys::readable_event_device_count() {
        Ok(count) if count > 0 => "Global autoclicker hotkeys enabled".to_string(),
        Ok(_) if crate::hotkeys::kde_global_shortcuts_available() => {
            "Global hotkeys enabled; Chrome override needs permissions".to_string()
        }
        Ok(_) if crate::hotkeys::portal_global_shortcuts_version().is_ok() => {
            "Global hotkeys enabled; Chrome override needs permissions".to_string()
        }
        Ok(_) => "Global hotkeys enabled, but permission is missing".to_string(),
        Err(err) => format!("Global hotkeys enabled, but input access failed: {err:#}"),
    }
}

fn cycle_local(state: &Rc<RefCell<UiState>>, status: &Label) {
    let mut state_ref = state.borrow_mut();
    let enabled: Vec<u8> = state_ref
        .config
        .clicker
        .cycle_order
        .iter()
        .copied()
        .filter(|slot_id| {
            state_ref
                .config
                .slot(*slot_id)
                .is_ok_and(|slot| slot.enabled)
        })
        .collect();

    if enabled.is_empty() {
        state_ref.local_slot = None;
        status.set_label("Stopped");
        return;
    }

    state_ref.local_slot = match state_ref.local_slot {
        None => Some(enabled[0]),
        Some(current) => {
            let Some(index) = enabled.iter().position(|slot_id| *slot_id == current) else {
                return;
            };
            if index + 1 >= enabled.len() {
                None
            } else {
                Some(enabled[index + 1])
            }
        }
    };
    state_ref.last_click = Instant::now();

    if let Some(slot_id) = state_ref.local_slot
        && let Ok(slot) = state_ref.config.slot(slot_id)
    {
        status.set_label(&format!("Testing {} at {} ms", slot.name, slot.interval_ms));
    } else {
        status.set_label("Stopped");
    }
}

fn add_pixel(state: &Rc<RefCell<UiState>>, x: f64, y: f64) {
    push_pixel(&mut state.borrow_mut(), x, y);
}

fn push_pixel(state: &mut UiState, x: f64, y: f64) {
    let now = Instant::now();
    state
        .dots
        .retain(|dot| now.saturating_duration_since(dot.created) < DOT_LIFETIME);
    if state.dots.len() >= MAX_PIXELS {
        let drain = state.dots.len() - MAX_PIXELS + 1;
        state.dots.drain(0..drain);
    }
    let color = match state.next_dot_color % 3 {
        0 => DotColor::Red,
        1 => DotColor::Yellow,
        _ => DotColor::Blue,
    };
    state.next_dot_color = state.next_dot_color.wrapping_add(1);
    state.dots.push(Dot {
        x: x.round(),
        y: y.round(),
        created: now,
        color,
    });
}

fn active_slot_rgb(name: &str) -> (f64, f64, f64) {
    match name {
        "Slow" => (0.10, 0.45, 0.95),
        "Fast" => (0.95, 0.55, 0.08),
        _ => (0.45, 0.32, 0.95),
    }
}

fn save_ui_config(config: &Config, status: &Label) {
    match config.save() {
        Ok(()) => {
            let _ = ipc::send(ipc::Request::Reload);
            status.set_label("Saved settings");
        }
        Err(err) => status.set_label(&format!("Save failed: {err:#}")),
    }
}
