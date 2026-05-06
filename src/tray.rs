use crate::ipc;

#[derive(Debug)]
struct AutolonTray;

impl ksni::Tray for AutolonTray {
    const MENU_ON_ACTIVATE: bool = true;

    fn id(&self) -> String {
        crate::config::APP_ID.to_string()
    }

    fn title(&self) -> String {
        "Autolon".to_string()
    }

    fn icon_name(&self) -> String {
        crate::config::APP_ID.to_string()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        vec![tray_icon(32)]
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            icon_name: crate::config::APP_ID.to_string(),
            icon_pixmap: self.icon_pixmap(),
            title: "Autolon".to_string(),
            description: "Autoclicker controls".to_string(),
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        spawn_gui();
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::*;
        vec![
            StandardItem {
                label: "Settings...".into(),
                icon_name: "preferences-system".into(),
                activate: Box::new(|_| spawn_gui()),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Quit".into(),
                icon_name: "application-exit".into(),
                activate: Box::new(|_| {
                    let _ = ipc::send(ipc::Request::Quit);
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}

pub fn spawn() {
    std::thread::spawn(|| {
        let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            eprintln!("autolon: failed to create tray runtime");
            return;
        };
        runtime.block_on(async {
            use ksni::TrayMethods;
            match AutolonTray.spawn().await {
                Ok(_handle) => std::future::pending::<()>().await,
                Err(err) => eprintln!("autolon: tray unavailable: {err}"),
            }
        });
    });
}

fn spawn_gui() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::process::Command::new(exe).arg("gui").spawn();
    }
}

fn tray_icon(size: i32) -> ksni::Icon {
    let size = size.max(16) as usize;
    let mut data = vec![0_u8; size * size * 4];

    for y in 0..size {
        for x in 0..size {
            let bg = x < 5 || y < 5 || x + 6 > size || y + 6 > size;
            if bg {
                put(&mut data, size, x, y, [0xff, 0x11, 0x18, 0x27]);
            }
        }
    }

    draw_line(
        &mut data,
        size,
        (7, 6),
        (18, 28),
        [0xff, 0xf8, 0xfa, 0xfc],
        2,
    );
    draw_line(
        &mut data,
        size,
        (18, 28),
        (21, 19),
        [0xff, 0xf8, 0xfa, 0xfc],
        2,
    );
    draw_line(
        &mut data,
        size,
        (21, 19),
        (30, 16),
        [0xff, 0xf8, 0xfa, 0xfc],
        2,
    );
    draw_line(
        &mut data,
        size,
        (30, 16),
        (7, 6),
        [0xff, 0xf8, 0xfa, 0xfc],
        2,
    );
    draw_line(
        &mut data,
        size,
        (21, 20),
        (30, 29),
        [0xff, 0x25, 0x63, 0xeb],
        2,
    );

    draw_line(&mut data, size, (5, 2), (7, 9), [0xff, 0xf5, 0x9e, 0x0b], 1);
    draw_line(
        &mut data,
        size,
        (2, 12),
        (9, 14),
        [0xff, 0xf5, 0x9e, 0x0b],
        1,
    );
    draw_line(
        &mut data,
        size,
        (14, 2),
        (11, 8),
        [0xff, 0xf5, 0x9e, 0x0b],
        1,
    );

    ksni::Icon {
        width: size as i32,
        height: size as i32,
        data,
    }
}

fn draw_line(
    data: &mut [u8],
    size: usize,
    from: (i32, i32),
    to: (i32, i32),
    argb: [u8; 4],
    radius: i32,
) {
    let (x0, y0) = from;
    let (x1, y1) = to;
    let dx = (x1 - x0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let dy = -(y1 - y0).abs();
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    let mut x = x0;
    let mut y = y0;

    loop {
        for yy in y - radius..=y + radius {
            for xx in x - radius..=x + radius {
                if xx >= 0 && yy >= 0 {
                    put(data, size, xx as usize, yy as usize, argb);
                }
            }
        }
        if x == x1 && y == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
    }
}

fn put(data: &mut [u8], size: usize, x: usize, y: usize, argb: [u8; 4]) {
    if x >= size || y >= size {
        return;
    }
    let offset = (y * size + x) * 4;
    data[offset..offset + 4].copy_from_slice(&argb);
}
