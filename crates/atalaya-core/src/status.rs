//! Estado del propio Atalaya: llaves, correo, rondas y servicio en segundo plano.

use crate::{Registry, checks, mail, secrets};
use anyhow::Result;
use serde_json::{Value, json};
use std::process::Command;

pub const SERVICE: &str = "cl.thrazos.atalaya.watch";

fn user_id() -> Option<String> {
    let out = Command::new("id").arg("-u").output().ok()?;
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// `Some(true)` si el vigía corre como servicio; `None` donde no se puede saber.
pub fn service_running() -> Option<bool> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let out = Command::new("launchctl")
        .args(["print", &format!("gui/{}/{SERVICE}", user_id()?)])
        .output()
        .ok()?;
    Some(out.status.success() && String::from_utf8_lossy(&out.stdout).contains("state = running"))
}

fn agent_file() -> Result<std::path::PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("no se encontró la carpeta de usuario"))?;
    Ok(home.join("Library/LaunchAgents").join(format!("{SERVICE}.plist")))
}

/// Instala el vigía como servicio de macOS: arranca al iniciar sesión y se reinicia
/// si se cae. Usa el ejecutable desde donde se llama, así que sirve tanto para la
/// instalación del paquete como para una copia compilada a mano.
pub fn install_service() -> Result<()> {
    if !cfg!(target_os = "macos") {
        anyhow::bail!("el servicio automático solo existe en macOS; en Linux usa systemd");
    }
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("no se encontró la carpeta de usuario"))?;
    let exe = std::env::current_exe()?;
    let logs = home.join("Library/Logs/Atalaya");
    std::fs::create_dir_all(&logs)?;
    let log = logs.join("watch.log");
    let file = agent_file()?;
    std::fs::create_dir_all(file.parent().expect("carpeta"))?;
    std::fs::write(
        &file,
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{SERVICE}</string>
  <key>ProgramArguments</key>
  <array><string>{exe}</string><string>watch</string></array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>ProcessType</key><string>Background</string>
  <key>StandardOutPath</key><string>{log}</string>
  <key>StandardErrorPath</key><string>{log}</string>
</dict>
</plist>
"#,
            exe = exe.display(),
            log = log.display(),
        ),
    )?;
    let domain = format!("gui/{}", user_id().unwrap_or_default());
    // Si ya estaba cargado (versión anterior), se descarga primero.
    let _ = Command::new("launchctl").args(["bootout", &format!("{domain}/{SERVICE}")]).output();
    let out = Command::new("launchctl")
        .args(["bootstrap", &domain])
        .arg(&file)
        .output()?;
    if !out.status.success() {
        anyhow::bail!("launchctl no pudo cargar el servicio: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(())
}

pub fn uninstall_service() -> Result<()> {
    let domain = format!("gui/{}", user_id().unwrap_or_default());
    let _ = Command::new("launchctl").args(["bootout", &format!("{domain}/{SERVICE}")]).output();
    let file = agent_file()?;
    if file.exists() {
        std::fs::remove_file(file)?;
    }
    Ok(())
}

pub fn system_status(registry: &Registry) -> Result<Value> {
    let keys: Vec<Value> = secrets::SERVICES
        .iter()
        .map(|s| {
            // GitHub también vale con la sesión del comando `gh`.
            let saved = if *s == "github" { crate::github::token().is_some() } else { secrets::get(s).is_some() };
            json!({ "service": s, "saved": saved })
        })
        .collect();
    let rounds: Vec<Value> = checks::KINDS
        .iter()
        .map(|(kind, every)| json!({ "kind": kind, "every_seconds": every }))
        .collect();
    Ok(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "database": Registry::default_path()?.display().to_string(),
        "service_running": service_running(),
        "keys": keys,
        "rounds": rounds,
        "email": {
            "address": registry.setting(mail::ADDRESS)?,
            "to": registry.setting(mail::TO)?,
            "report_hour": registry.setting(mail::REPORT_HOUR)?.and_then(|h| h.parse::<u32>().ok()).unwrap_or(8),
            "ready": mail::configured(registry)?,
        },
        "flags_store": registry.setting("flags_store_id")?,
        "auto_merge": registry.setting(crate::worker::AUTO_MERGE)?.as_deref() == Some("si"),
    }))
}

/// Comprobación del sistema: todo lo que Atalaya necesita para no trabarse a mitad de
/// camino. Al leer la carpeta de cada proyecto, macOS muestra de una vez todos los
/// avisos de permiso pendientes, en vez de pedirlos de a uno cuando ya hay un trabajo
/// en marcha. Cada elemento: nombre, si está bien, detalle y cómo arreglarlo.
pub fn doctor(registry: &Registry) -> Result<Value> {
    let mut items: Vec<Value> = Vec::new();
    let mut add = |name: &str, ok: bool, detail: String, fix: &str| {
        items.push(json!({ "name": name, "ok": ok, "detail": detail, "fix": if ok { "" } else { fix } }));
    };

    // Carpetas propias.
    let data = Registry::default_path()?;
    let data_dir = data.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    let probe = data_dir.join(".prueba-escritura");
    let writable = std::fs::write(&probe, b"ok").is_ok();
    let _ = std::fs::remove_file(&probe);
    add("Carpeta de datos", writable, data_dir.display().to_string(), "Revisa los permisos de esa carpeta.");
    let work = crate::worker::worktrees_dir()?;
    add(
        "Carpeta de trabajo del agente",
        std::fs::create_dir_all(&work).is_ok(),
        work.display().to_string(),
        "Revisa los permisos de esa carpeta.",
    );

    // Acceso a cada proyecto: es lo que macOS protege con avisos.
    for project in registry.list()? {
        let Some(path) = project.local_path else { continue };
        let readable = std::fs::read_dir(&path).map(|mut d| d.next().is_some()).unwrap_or(false);
        add(
            &format!("Acceso a {}", project.name),
            readable,
            path,
            "Acepta el aviso de macOS, o da acceso en Ajustes del Sistema → Privacidad y seguridad → Archivos y carpetas → Atalaya.",
        );
    }

    // Herramientas que usan las revisiones y el agente.
    let path = crate::worker::developer_path();
    let find = |tool: &str| std::env::split_paths(&path).map(|d| d.join(tool)).find(|p| p.is_file());
    for (tool, purpose, fix) in [
        ("git", "historial, copias de trabajo y subida de ramas", "Instala las herramientas de línea de comandos de Xcode: xcode-select --install"),
        ("npm", "revisión de dependencias y trabajo del agente", "Instala Node.js desde nodejs.org o con Homebrew."),
        ("openssl", "revisión de certificados", "Viene con macOS; revisa tu PATH."),
        ("gh", "acceso a GitHub sin llave propia", "Opcional: instala GitHub CLI o agrega la conexión de GitHub en Conexiones."),
    ] {
        let found = find(tool);
        add(
            &format!("Herramienta {tool}"),
            found.is_some() || tool == "gh" && crate::secrets::get("github").is_some(),
            found.map(|p| p.display().to_string()).unwrap_or_else(|| format!("no encontrada; se usa para {purpose}")),
            fix,
        );
    }
    // Cada rol de IA necesita al menos un candidato listo. El revisor vacío es una elección válida.
    for (role, label, _) in crate::orchestrator::ROLES {
        let configured = crate::orchestrator::config(registry, role).map(|c| c.candidates).unwrap_or_default();
        let (ready, skipped) = crate::orchestrator::lineup(registry, role, &[]).unwrap_or_default();
        let (ok, detail) = match ready.first() {
            Some(first) => (true, format!("lo hace {}", first.label())),
            None if configured.is_empty() && *role == "revisor" => (true, "sin revisor: los cambios llegan sin revisión cruzada".into()),
            None if configured.is_empty() => (false, "nadie asignado".into()),
            None => (false, format!("nadie disponible: {}", skipped.join(" · "))),
        };
        add(
            &format!("IA: {label}"),
            ok,
            detail,
            "Elige quién lo hace en IA. Claude Code se instala aparte e inicia sesión con el comando claude; los modelos y agentes se agregan en Conexiones.",
        );
    }

    // Conexiones: si el Llavero pide permiso, lo pide aquí.
    let _ = crate::connections::adopt_legacy(registry);
    let saved = crate::connections::all(registry).unwrap_or_default();
    if saved.is_empty() {
        add(
            "Conexiones",
            false,
            "ninguna todavía; Atalaya solo vigila carpetas y sitios".into(),
            "Opcional: agrega tus repos, hosting, bases e IA en Conexiones.",
        );
    }
    for c in saved {
        let needs = crate::connections::provider(&c.provider).is_some_and(|p| p.secret.is_some());
        let ok = !needs || c.secret().is_some();
        add(
            &format!("Conexión {}", c.name),
            ok,
            if ok { "lista".into() } else { "falta su llave en el Llavero".to_string() },
            "Opcional: edítala en Conexiones y vuelve a pegar la llave.",
        );
    }

    let running = service_running();
    add(
        "Vigía en segundo plano",
        running != Some(false),
        match running {
            Some(true) => "en guardia".into(),
            Some(false) => "detenido".into(),
            None => "no aplica en este sistema".into(),
        },
        "Instálalo con: atalaya service install",
    );
    Ok(Value::Array(items))
}
