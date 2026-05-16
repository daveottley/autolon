use crate::{
    clicker::{ClickerState, Status},
    config::{Config, MIN_GLOBAL_HOTKEY_DEBOUNCE_MS},
    ipc,
};
use anyhow::Result;
use gtk4::gdk;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{
    Align, Application, ApplicationWindow, Box as GtkBox, Button, CheckButton, DrawingArea,
    EventControllerKey, EventControllerMotion, GestureClick, Label, Orientation, SpinButton,
};
use std::cell::RefCell;
use std::io::{ErrorKind, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::rc::Rc;
use std::thread;
use std::time::{Duration, Instant};

const CANVAS_WIDTH: i32 = 680;
const CANVAS_HEIGHT: i32 = 360;
const WINDOW_WIDTH: i32 = 760;
const WINDOW_HEIGHT: i32 = 700;
const MAX_PIXELS: usize = 60_000;
const DOT_SIZE: f64 = 5.0;
const DOT_LIFETIME: Duration = Duration::from_secs(4);
const ACTIVE_RING_RADIUS: f64 = 13.0;
const SAVE_DEBOUNCE: Duration = Duration::from_millis(200);

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
    pointer_inside_canvas: bool,
    local_slot: Option<u8>,
    global_slot: Option<GlobalSlot>,
    capture_target: Option<CaptureTarget>,
    last_click: Instant,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CaptureTarget {
    Cycle,
    Stop,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct GlobalSlot {
    slot_id: u8,
    interval_ms: u64,
}

fn build_ui(app: &Application) {
    let config = Config::load_or_create().unwrap_or_default();
    let state = Rc::new(RefCell::new(UiState {
        config,
        dots: Vec::new(),
        next_dot_color: 0,
        pointer: (CANVAS_WIDTH as f64 / 2.0, CANVAS_HEIGHT as f64 / 2.0),
        pointer_inside_canvas: false,
        local_slot: None,
        global_slot: None,
        capture_target: None,
        last_click: Instant::now(),
    }));
    let pending_save = Rc::new(RefCell::new(None));
    subscribe_to_shutdown(app, state.clone(), pending_save.clone());

    let root = GtkBox::new(Orientation::Vertical, 14);
    root.set_margin_top(16);
    root.set_margin_bottom(16);
    root.set_margin_start(16);
    root.set_margin_end(16);

    let header = Label::new(None);
    header.set_markup("<b><big>Global Autoclicker</big></b>");
    header.set_xalign(0.5);
    root.append(&header);

    let hotkeys_title = Label::new(None);
    update_hotkeys_title(&hotkeys_title, &state);
    hotkeys_title.set_xalign(0.5);
    root.append(&hotkeys_title);
    let canvas_notice = Label::new(Some(&test_canvas_notice(&state.borrow().config)));
    canvas_notice.set_xalign(0.5);
    canvas_notice.set_wrap(true);

    let enable_global = Button::with_label(if state.borrow().config.global_autoclicker_enabled {
        "Disable Global Autoclicker"
    } else {
        "Enable Global Autoclicker"
    });
    root.append(&enable_global);

    let settings_header = Label::new(None);
    settings_header.set_markup("<b><big>Settings</big></b>");
    settings_header.set_xalign(0.5);
    root.append(&settings_header);

    let cycle_hotkey = Button::with_label(&state.borrow().config.hotkeys.cycle);
    let stop_hotkey = Button::with_label(&state.borrow().config.hotkeys.stop);
    let debounce_help = Label::new(None);
    append_global_overlay_setting(&root, state.clone(), pending_save.clone());
    append_hotkey_controls(
        &root,
        state.clone(),
        hotkeys_title.clone(),
        pending_save.clone(),
        cycle_hotkey.clone(),
        stop_hotkey.clone(),
        canvas_notice.clone(),
        debounce_help.clone(),
    );

    let control_grid = GtkBox::new(Orientation::Horizontal, 34);
    control_grid.set_hexpand(true);
    let slot_controls = GtkBox::new(Orientation::Vertical, 8);
    for slot_id in [1_u8, 2, 3] {
        append_slot(
            &slot_controls,
            state.clone(),
            hotkeys_title.clone(),
            pending_save.clone(),
            slot_id,
        );
    }
    control_grid.append(&slot_controls);
    let spacer = GtkBox::new(Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    control_grid.append(&spacer);
    append_hotkey_debounce(
        &control_grid,
        state.clone(),
        pending_save.clone(),
        debounce_help.clone(),
    );
    root.append(&control_grid);

    let canvas_header = Label::new(None);
    canvas_header.set_markup("<b><big>Test Canvas</big></b>");
    canvas_header.set_xalign(0.5);
    root.append(&canvas_header);

    root.append(&canvas_notice);

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
            if let Some((slot_id, interval_ms)) = active_indicator(&state_ref) {
                let slot_name = state_ref
                    .config
                    .slot(slot_id)
                    .map(|slot| slot.name.as_str())
                    .unwrap_or("User");
                let interval_text = interval_ms.to_string();
                let (red, green, blue) = active_slot_rgb(slot_name);
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
                cr.select_font_face(
                    "Sans",
                    gtk4::cairo::FontSlant::Normal,
                    gtk4::cairo::FontWeight::Bold,
                );
                cr.set_font_size(10.0);
                cr.set_source_rgba(red, green, blue, 0.86);
                cr.move_to(
                    x.clamp(ACTIVE_RING_RADIUS, width as f64 - ACTIVE_RING_RADIUS) + 16.0,
                    y.clamp(ACTIVE_RING_RADIUS, height as f64 - ACTIVE_RING_RADIUS) + 4.0,
                );
                let _ = cr.show_text(&interval_text);
            }
        }
    });
    subscribe_to_global_status(state.clone(), canvas.clone(), hotkeys_title.clone());

    let motion = EventControllerMotion::new();
    motion.connect_motion({
        let state = state.clone();
        let canvas = canvas.clone();
        move |_, x, y| {
            let mut state_ref = state.borrow_mut();
            state_ref.pointer = (x, y);
            state_ref.pointer_inside_canvas = true;
            canvas.queue_draw();
        }
    });
    motion.connect_leave({
        let state = state.clone();
        let canvas = canvas.clone();
        move |_| {
            state.borrow_mut().pointer_inside_canvas = false;
            canvas.queue_draw();
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
        let enable_global = enable_global.clone();
        let hotkeys_title = hotkeys_title.clone();
        let pending_save = pending_save.clone();
        let canvas = canvas.clone();
        move |_| {
            let new_enabled = {
                let mut state_ref = state.borrow_mut();
                let new_enabled = !state_ref.config.global_autoclicker_enabled;
                state_ref.config.global_autoclicker_enabled = new_enabled;
                new_enabled
            };
            if new_enabled {
                enable_global.set_label("Disable Global Autoclicker");
            } else {
                enable_global.set_label("Enable Global Autoclicker");
            }
            update_hotkeys_title(&hotkeys_title, &state);
            schedule_config_save(&state, &pending_save);
            canvas.grab_focus();
        }
    });

    let app_for_quit = app.clone();
    quit.connect_clicked({
        let state = state.clone();
        let pending_save = pending_save.clone();
        move |_| {
            flush_pending_save(&state, &pending_save, false);
            let _ = ipc::send(ipc::Request::Quit);
            app_for_quit.quit();
        }
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
                && state_ref.pointer_inside_canvas
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
        .default_width(WINDOW_WIDTH)
        .default_height(WINDOW_HEIGHT)
        .resizable(false)
        .child(&root)
        .build();
    window.set_size_request(WINDOW_WIDTH, WINDOW_HEIGHT);
    let key = EventControllerKey::new();
    key.connect_key_pressed({
        let state = state.clone();
        let hotkeys_title = hotkeys_title.clone();
        let cycle_hotkey = cycle_hotkey.clone();
        let stop_hotkey = stop_hotkey.clone();
        let pending_save = pending_save.clone();
        let canvas_notice = canvas_notice.clone();
        let debounce_help = debounce_help.clone();
        let canvas = canvas.clone();
        move |_, key, _, _| {
            if handle_hotkey_capture(
                key,
                &state,
                &hotkeys_title,
                &cycle_hotkey,
                &stop_hotkey,
                &canvas_notice,
                &debounce_help,
                &pending_save,
            ) {
                return glib::Propagation::Stop;
            }
            let (cycle, stop) = {
                let state_ref = state.borrow();
                (
                    state_ref.config.hotkeys.cycle.clone(),
                    state_ref.config.hotkeys.stop.clone(),
                )
            };
            let Some(label) = function_key_from_gdk(key) else {
                return glib::Propagation::Proceed;
            };
            if label == cycle {
                cycle_local(&state, &hotkeys_title);
                canvas.queue_draw();
                glib::Propagation::Stop
            } else if label == stop {
                state.borrow_mut().local_slot = None;
                update_hotkeys_title(&hotkeys_title, &state);
                canvas.queue_draw();
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        }
    });
    window.add_controller(key);
    window.connect_is_active_notify({
        let state = state.clone();
        let hotkeys_title = hotkeys_title.clone();
        let canvas = canvas.clone();
        move |window| {
            if !window.is_active() && state.borrow().local_slot.is_some() {
                state.borrow_mut().local_slot = None;
                update_hotkeys_title(&hotkeys_title, &state);
                canvas.queue_draw();
            }
        }
    });
    window.connect_close_request({
        let state = state.clone();
        let pending_save = pending_save.clone();
        move |_| {
            flush_pending_save(&state, &pending_save, true);
            glib::Propagation::Proceed
        }
    });
    window.set_icon_name(Some(crate::config::APP_ID));
    window.present();
    canvas.grab_focus();
}

fn append_slot(
    root: &GtkBox,
    state: Rc<RefCell<UiState>>,
    hotkeys_title: Label,
    pending_save: Rc<RefCell<Option<glib::SourceId>>>,
    slot_id: u8,
) {
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
        let hotkeys_title = hotkeys_title.clone();
        let pending_save = pending_save.clone();
        move |enabled| {
            let mut state_ref = state.borrow_mut();
            if let Ok(slot) = state_ref.config.slot_mut(slot_id) {
                slot.enabled = enabled.is_active();
            }
            if !enabled.is_active() && state_ref.local_slot == Some(slot_id) {
                state_ref.local_slot = None;
            }
            drop(state_ref);
            update_hotkeys_title(&hotkeys_title, &state);
            schedule_config_save(&state, &pending_save);
        }
    });

    speed.connect_value_changed({
        let state = state.clone();
        let hotkeys_title = hotkeys_title.clone();
        let pending_save = pending_save.clone();
        move |speed| {
            let mut state_ref = state.borrow_mut();
            if let Ok(slot) = state_ref.config.slot_mut(slot_id) {
                slot.interval_ms = speed.value() as u64;
                if slot.press_duration_ms >= slot.interval_ms {
                    slot.press_duration_ms = slot.interval_ms.saturating_sub(1).max(1);
                }
            }
            drop(state_ref);
            update_hotkeys_title(&hotkeys_title, &state);
            schedule_config_save(&state, &pending_save);
        }
    });
}

fn append_global_overlay_setting(
    root: &GtkBox,
    state: Rc<RefCell<UiState>>,
    pending_save: Rc<RefCell<Option<glib::SourceId>>>,
) {
    let group = GtkBox::new(Orientation::Vertical, 4);
    let checkbox = CheckButton::with_label("Display Global Mouse Overlay");
    checkbox.set_active(state.borrow().config.display_global_mouse_overlay);

    let warning = Label::new(None);
    warning.set_xalign(0.0);
    warning.set_wrap(true);
    warning.set_markup("<span foreground=\"#dc2626\"><b>Warning:</b></span> This feature is only imlemented in the KDE destop environment. Using it on other system may cause unwanted visual artifacts or break your display system. USE WITH CAUTION!");
    warning.set_visible(checkbox.is_active());

    group.append(&checkbox);
    group.append(&warning);
    root.append(&group);

    checkbox.connect_toggled(move |checkbox| {
        state.borrow_mut().config.display_global_mouse_overlay = checkbox.is_active();
        warning.set_visible(checkbox.is_active());
        schedule_config_save(&state, &pending_save);
    });
}

fn append_hotkey_controls(
    root: &GtkBox,
    state: Rc<RefCell<UiState>>,
    hotkeys_title: Label,
    pending_save: Rc<RefCell<Option<glib::SourceId>>>,
    cycle_hotkey: Button,
    stop_hotkey: Button,
    canvas_notice: Label,
    debounce_help: Label,
) {
    let row = GtkBox::new(Orientation::Horizontal, 10);
    let cycle_label = Label::new(Some("Cycle Autoclick Speed"));
    cycle_label.set_xalign(0.0);
    let stop_label = Label::new(Some("Emergency Stop"));
    stop_label.set_xalign(0.0);
    row.append(&cycle_label);
    row.append(&cycle_hotkey);
    row.append(&stop_label);
    row.append(&stop_hotkey);
    root.append(&row);

    cycle_hotkey.connect_clicked({
        let state = state.clone();
        let cycle_hotkey = cycle_hotkey.clone();
        move |_| {
            state.borrow_mut().capture_target = Some(CaptureTarget::Cycle);
            cycle_hotkey.set_label("Press key...");
        }
    });
    stop_hotkey.connect_clicked({
        let state = state.clone();
        let stop_hotkey = stop_hotkey.clone();
        move |_| {
            state.borrow_mut().capture_target = Some(CaptureTarget::Stop);
            stop_hotkey.set_label("Press key...");
        }
    });

    update_hotkey_buttons(&state, &cycle_hotkey, &stop_hotkey);
    update_hotkeys_title(&hotkeys_title, &state);
    canvas_notice.set_label(&test_canvas_notice(&state.borrow().config));
    update_debounce_help(&debounce_help, &state.borrow().config);
    let _ = pending_save;
}

fn append_hotkey_debounce(
    root: &GtkBox,
    state: Rc<RefCell<UiState>>,
    pending_save: Rc<RefCell<Option<glib::SourceId>>>,
    help: Label,
) {
    let group = GtkBox::new(Orientation::Vertical, 6);
    group.set_hexpand(false);
    group.set_halign(Align::End);
    group.set_width_request(300);
    let label = Label::new(Some("Hotkey debounce"));
    label.set_xalign(0.0);
    let row = GtkBox::new(Orientation::Horizontal, 10);
    let debounce = SpinButton::with_range(MIN_GLOBAL_HOTKEY_DEBOUNCE_MS as f64, 1000.0, 1.0);
    debounce.set_value(state.borrow().config.global_hotkey_debounce_ms as f64);
    let ms = Label::new(Some("ms"));
    update_debounce_help(&help, &state.borrow().config);
    help.set_xalign(0.0);
    help.set_wrap(true);
    row.append(&label);
    row.append(&debounce);
    row.append(&ms);
    group.append(&row);
    group.append(&help);
    root.append(&group);

    debounce.connect_value_changed(move |debounce| {
        {
            state.borrow_mut().config.global_hotkey_debounce_ms = debounce.value() as u64;
        }
        schedule_config_save(&state, &pending_save);
    });
}

fn handle_hotkey_capture(
    key: gdk::Key,
    state: &Rc<RefCell<UiState>>,
    hotkeys_title: &Label,
    cycle_hotkey: &Button,
    stop_hotkey: &Button,
    canvas_notice: &Label,
    debounce_help: &Label,
    pending_save: &Rc<RefCell<Option<glib::SourceId>>>,
) -> bool {
    let target = state.borrow().capture_target;
    let Some(target) = target else {
        return false;
    };
    if key == gdk::Key::Escape {
        state.borrow_mut().capture_target = None;
        update_hotkey_buttons(state, cycle_hotkey, stop_hotkey);
        return true;
    }
    let Some(label) = function_key_from_gdk(key) else {
        return true;
    };

    {
        let mut state_ref = state.borrow_mut();
        match target {
            CaptureTarget::Cycle => state_ref.config.hotkeys.cycle = label,
            CaptureTarget::Stop => state_ref.config.hotkeys.stop = label,
        }
        state_ref.capture_target = None;
    }
    update_hotkey_buttons(state, cycle_hotkey, stop_hotkey);
    update_hotkeys_title(hotkeys_title, state);
    canvas_notice.set_label(&test_canvas_notice(&state.borrow().config));
    update_debounce_help(debounce_help, &state.borrow().config);
    schedule_config_save(state, pending_save);
    true
}

fn update_hotkey_buttons(
    state: &Rc<RefCell<UiState>>,
    cycle_hotkey: &Button,
    stop_hotkey: &Button,
) {
    let state_ref = state.borrow();
    cycle_hotkey.set_label(&state_ref.config.hotkeys.cycle);
    stop_hotkey.set_label(&state_ref.config.hotkeys.stop);
}

fn function_key_from_gdk(key: gdk::Key) -> Option<String> {
    match key {
        gdk::Key::F1 => Some("F1".to_string()),
        gdk::Key::F2 => Some("F2".to_string()),
        gdk::Key::F3 => Some("F3".to_string()),
        gdk::Key::F4 => Some("F4".to_string()),
        gdk::Key::F5 => Some("F5".to_string()),
        gdk::Key::F6 => Some("F6".to_string()),
        gdk::Key::F7 => Some("F7".to_string()),
        gdk::Key::F8 => Some("F8".to_string()),
        gdk::Key::F9 => Some("F9".to_string()),
        gdk::Key::F10 => Some("F10".to_string()),
        gdk::Key::F11 => Some("F11".to_string()),
        gdk::Key::F12 => Some("F12".to_string()),
        _ => None,
    }
}

fn update_hotkeys_title(label: &Label, state: &Rc<RefCell<UiState>>) {
    let state_ref = state.borrow();
    if state_ref.config.global_autoclicker_enabled {
        let running = state_ref
            .global_slot
            .and_then(|global_slot| {
                state_ref
                    .config
                    .slot(global_slot.slot_id)
                    .ok()
                    .map(|slot| (slot.name.clone(), global_slot.interval_ms))
            })
            .map(|(name, interval_ms)| {
                format!(
                    " (Running {} at {interval_ms} ms)",
                    glib::markup_escape_text(&name).to_ascii_lowercase()
                )
            })
            .unwrap_or_else(|| " (not running)".to_string());
        label.set_markup(&format!(
            "<b><big>Hotkeys</big></b>: <span foreground=\"#15803d\"><b>enabled</b></span>{running}"
        ));
        return;
    }

    let testing = if let Some(slot_id) = state_ref.local_slot
        && let Ok(slot) = state_ref.config.slot(slot_id)
    {
        let name = glib::markup_escape_text(&slot.name);
        format!("testing {name} at {} ms", slot.interval_ms)
    } else {
        "testing stopped".to_string()
    };
    label.set_markup(&format!(
        "<b><big>Hotkeys</big></b>: <span foreground=\"#b45309\"><b>disabled</b></span> ({testing})"
    ));
}

fn test_canvas_notice(config: &Config) -> String {
    format!(
        "Try playing with your autoclicker settings here before using it outside.\nRemember you can use {} to Emergency Stop.",
        config.hotkeys.stop
    )
}

fn update_debounce_help(label: &Label, config: &Config) {
    label.set_label(&format!(
        "Minimum time between accepted global {}/{} presses; lower cycles faster but risks double-presses.",
        config.hotkeys.cycle, config.hotkeys.stop
    ));
}

fn cycle_local(state: &Rc<RefCell<UiState>>, hotkeys_title: &Label) {
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
        drop(state_ref);
        update_hotkeys_title(hotkeys_title, state);
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

    drop(state_ref);
    update_hotkeys_title(hotkeys_title, state);
}

fn add_pixel(state: &Rc<RefCell<UiState>>, x: f64, y: f64) {
    push_pixel(&mut state.borrow_mut(), x, y);
}

fn active_indicator(state: &UiState) -> Option<(u8, u64)> {
    if !state.pointer_inside_canvas {
        return None;
    }
    if state.config.global_autoclicker_enabled {
        return state
            .global_slot
            .map(|slot| (slot.slot_id, slot.interval_ms));
    }
    let slot_id = state.local_slot?;
    let interval_ms = state.config.slot(slot_id).ok()?.interval_ms;
    Some((slot_id, interval_ms))
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

fn schedule_config_save(
    state: &Rc<RefCell<UiState>>,
    pending_save: &Rc<RefCell<Option<glib::SourceId>>>,
) {
    if let Some(source_id) = pending_save.borrow_mut().take() {
        source_id.remove();
    }
    let config = state.borrow().config.clone();
    let pending_save_for_timeout = pending_save.clone();
    let source_id = glib::timeout_add_local_once(SAVE_DEBOUNCE, move || {
        *pending_save_for_timeout.borrow_mut() = None;
        thread::spawn(move || {
            if let Err(err) = save_and_reload(config) {
                eprintln!("autolon: failed to save settings: {err:#}");
            }
        });
    });
    *pending_save.borrow_mut() = Some(source_id);
}

fn flush_pending_save(
    state: &Rc<RefCell<UiState>>,
    pending_save: &Rc<RefCell<Option<glib::SourceId>>>,
    reload_daemon: bool,
) {
    let Some(source_id) = pending_save.borrow_mut().take() else {
        return;
    };
    source_id.remove();
    let result = if reload_daemon {
        save_and_reload(state.borrow().config.clone())
    } else {
        state.borrow().config.save()
    };
    if let Err(err) = result {
        eprintln!("autolon: failed to save settings: {err:#}");
    }
}

fn save_and_reload(config: Config) -> Result<()> {
    config.save()?;
    let _ = ipc::send(ipc::Request::Reload);
    Ok(())
}

fn subscribe_to_global_status(
    state: Rc<RefCell<UiState>>,
    canvas: DrawingArea,
    hotkeys_title: Label,
) {
    let Ok((mut status_reader, mut status_writer)) = UnixStream::pair() else {
        eprintln!("autolon: failed to create GUI status pipe");
        return;
    };
    if let Err(err) = status_reader.set_nonblocking(true) {
        eprintln!("autolon: failed to configure GUI status pipe: {err}");
        return;
    }

    let fd = status_reader.as_raw_fd();
    let buffer = Rc::new(RefCell::new(String::new()));
    glib::unix_fd_add_local(fd, glib::IOCondition::IN, {
        let buffer = buffer.clone();
        move |_, _| {
            let mut chunk = [0_u8; 4096];
            loop {
                match status_reader.read(&mut chunk) {
                    Ok(0) => return glib::ControlFlow::Break,
                    Ok(n) => {
                        buffer
                            .borrow_mut()
                            .push_str(&String::from_utf8_lossy(&chunk[..n]));
                    }
                    Err(err) if err.kind() == ErrorKind::WouldBlock => break,
                    Err(_) => return glib::ControlFlow::Break,
                }
            }

            let mut redraw = false;
            loop {
                let line = {
                    let mut buffer_ref = buffer.borrow_mut();
                    let Some(index) = buffer_ref.find('\n') else {
                        break;
                    };
                    buffer_ref.drain(..=index).collect::<String>()
                };
                if let Ok(status) = serde_json::from_str::<Status>(line.trim_end()) {
                    redraw |= update_global_status(&state, status);
                }
            }
            if redraw {
                update_hotkeys_title(&hotkeys_title, &state);
                canvas.queue_draw();
            }
            glib::ControlFlow::Continue
        }
    });

    thread::spawn(move || {
        let result = ipc::subscribe_status(move |status| {
            let line = serde_json::to_string(&status)?;
            writeln!(status_writer, "{line}")?;
            Ok(())
        });
        if let Err(err) = result {
            eprintln!("autolon: status subscription ended: {err:#}");
        }
    });
}

fn update_global_status(state: &Rc<RefCell<UiState>>, status: Status) -> bool {
    let next = match status.state {
        ClickerState::Running {
            slot_id,
            interval_ms,
            ..
        } => Some(GlobalSlot {
            slot_id,
            interval_ms,
        }),
        ClickerState::Stopped => None,
    };
    let mut state_ref = state.borrow_mut();
    if state_ref.global_slot == next {
        return false;
    }
    state_ref.global_slot = next;
    true
}

fn subscribe_to_shutdown(
    app: &Application,
    state: Rc<RefCell<UiState>>,
    pending_save: Rc<RefCell<Option<glib::SourceId>>>,
) {
    let Ok((mut shutdown_reader, mut shutdown_writer)) = UnixStream::pair() else {
        eprintln!("autolon: failed to create GUI shutdown pipe");
        return;
    };
    let fd = shutdown_reader.as_raw_fd();
    let app = app.clone();
    glib::unix_fd_add_local(fd, glib::IOCondition::IN, move |_, _| {
        let mut buffer = [0_u8; 8];
        let _ = shutdown_reader.read(&mut buffer);
        flush_pending_save(&state, &pending_save, false);
        app.quit();
        glib::ControlFlow::Break
    });

    thread::spawn(move || {
        if ipc::subscribe_shutdown().is_ok() {
            let _ = shutdown_writer.write_all(&[1]);
        }
    });
}
