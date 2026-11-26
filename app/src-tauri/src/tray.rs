//! Ícono de Atalaya en la barra de menú: muestra cuántas alertas hay abiertas
//! y da accesos rápidos sin abrir la ventana.

use atalaya_core::Registry;
use std::time::Duration;
use tauri::image::Image;
use tauri::menu::{MenuBuilder, MenuItemBuilder};
use tauri::tray::TrayIconBuilder;
use tauri::{App, AppHandle, Manager};

/// Alertas abiertas: (fallas, avisos) sin reconocer.
fn open_alerts() -> anyhow::Result<(usize, usize)> {
    let registry = Registry::open_default()?;
    let (mut fails, mut warns) = (0, 0);
    for project in registry.list()? {
        for check in registry.latest_checks(&project.slug)? {
            match (check.acked, check.status.as_str()) {
                (false, "falla") => fails += 1,
                (false, "aviso") => warns += 1,
                _ => {}
            }
        }
    }
    Ok((fails, warns))
}

pub fn show_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

pub fn setup(app: &App) -> tauri::Result<()> {
    let status = MenuItemBuilder::with_id("status", "Leyendo estado…").enabled(false).build(app)?;
    let open = MenuItemBuilder::with_id("open", "Abrir Atalaya").build(app)?;
    let check = MenuItemBuilder::with_id("check", "Revisar todo ahora").build(app)?;
    let quit = MenuItemBuilder::with_id("quit", "Salir de Atalaya").build(app)?;
    let menu = MenuBuilder::new(app)
        .item(&status)
        .separator()
        .items(&[&open, &check])
        .separator()
        .item(&quit)
        .build()?;

    let tray = TrayIconBuilder::with_id("atalaya")
        .icon(Image::from_bytes(include_bytes!("../icons/tray.png"))?)
        // Monocromo: macOS lo pinta claro u oscuro según la barra.
        .icon_as_template(true)
        .tooltip("Atalaya")
        .menu(&menu)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => show_window(app),
            "check" => {
                // Las revisiones tardan; corren aparte y el conteo se actualiza solo.
                std::thread::spawn(|| {
                    if let Ok(registry) = Registry::open_default() {
                        let _ = atalaya_core::checks::run_due(&registry, None, true);
                    }
                });
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .build(app)?;

    // El vigía escribe en la base de datos; aquí solo se refleja el conteo.
    std::thread::spawn(move || {
        loop {
            let (title, text) = match open_alerts() {
                Ok((0, 0)) => (None, "Sin alertas abiertas".to_string()),
                Ok((fails, warns)) => {
                    let mut parts = Vec::new();
                    if fails > 0 {
                        parts.push(format!("{fails} {}", if fails == 1 { "falla" } else { "fallas" }));
                    }
                    if warns > 0 {
                        parts.push(format!("{warns} {}", if warns == 1 { "aviso" } else { "avisos" }));
                    }
                    (Some((fails + warns).to_string()), format!("{} sin reconocer", parts.join(" y ")))
                }
                Err(_) => (None, "No se pudo leer el estado".to_string()),
            };
            let _ = tray.set_title(title);
            let _ = status.set_text(text);
            std::thread::sleep(Duration::from_secs(20));
        }
    });
    Ok(())
}
