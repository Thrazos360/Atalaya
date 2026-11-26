//! Agentes de terminal con tu propia cuenta: Claude Code, Codex, Antigravity CLI, Gemini
//! CLI, Qwen Code, Kimi Code, Copilot, Cursor, OpenCode, Droid y Amp.
//!
//! Atalaya hace de puente, como un lanzador: no pide llaves de API. Cada agente inicia
//! sesión con su propio programa (que abre el navegador o la app del proveedor) y usa la
//! suscripción que ya tienes. Atalaya solo:
//! - detecta si está instalado y si tiene la sesión iniciada, preguntándole al programa
//!   o mirando si existe su archivo de sesión (nunca lo lee);
//! - abre la Terminal con el comando de instalación o de inicio de sesión, para que lo
//!   veas y lo completes tú;
//! - lo lanza dentro de su encierro para responder o trabajar.

use crate::Registry;
use crate::connections::Connection;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Cómo se sabe si un agente tiene la sesión iniciada.
#[derive(Debug, Clone, Copy)]
pub enum Session {
    /// Se pregunta al programa: (argumentos, cómo leer la respuesta).
    Ask(&'static [&'static str], fn(bool, &str) -> bool),
    /// Existe alguno de estos archivos (rutas desde la carpeta personal). No se leen.
    Files(&'static [&'static str]),
    /// Esta carpeta tiene algún archivo de sesión (sin contar las subcarpetas nombradas).
    /// Solo se miran los nombres.
    Folder(&'static str, &'static [&'static str]),
    /// No hay forma de saberlo sin usarlo.
    Unknown,
}

#[derive(Debug, Clone, Serialize)]
pub struct Cli {
    /// El id del proveedor de conexión (`connections::PROVIDERS`).
    pub id: &'static str,
    pub label: &'static str,
    /// Con qué cuenta funciona.
    pub account: &'static str,
    /// Nombres posibles del programa.
    #[serde(skip)]
    pub bins: &'static [&'static str],
    /// Comando para instalarlo, si hay uno de una línea.
    pub install: Option<&'static str>,
    pub url: &'static str,
    /// Comando que inicia la sesión (se abre en la Terminal).
    pub login: &'static str,
    /// Qué hacer en la Terminal para terminar de iniciar sesión.
    pub login_note: &'static str,
    #[serde(skip)]
    pub session: Session,
    /// Carpetas propias donde guarda su sesión y configuración: puede escribir ahí.
    #[serde(skip)]
    pub dirs: &'static [&'static str],
    /// Versiones antiguas con el mismo nombre de programa que ya no funcionan: (parte de
    /// su ruta real, nombre). Se ignoran y se avisa que hay que cambiarlas.
    #[serde(skip)]
    pub legacy: &'static [(&'static str, &'static str)],
    /// Cómo se usa gratis, si se puede. Los gratis van primero en la app.
    pub free: Option<&'static str>,
}

fn claude_logged_in(ok: bool, out: &str) -> bool {
    ok && serde_json::from_str::<serde_json::Value>(out.trim()).is_ok_and(|v| v["loggedIn"] == true)
}

fn exit_ok(ok: bool, _out: &str) -> bool {
    ok
}

fn amp_has_account(ok: bool, out: &str) -> bool {
    ok && out.contains("(user_")
}

fn opencode_has_credentials(ok: bool, out: &str) -> bool {
    // Termina con «N credentials»; con 0 no hay ninguna.
    ok && out.lines().filter_map(|l| l.trim_start_matches(|c: char| !c.is_ascii_digit()).split_whitespace().next()?.parse::<u32>().ok().filter(|_| l.contains("credential"))).any(|n| n > 0)
}

fn says_logged_in(ok: bool, out: &str) -> bool {
    let text = out.to_lowercase();
    ok && text.contains("logged in") && !text.contains("not logged in")
}

pub static CATALOG: &[Cli] = &[
    Cli {
        id: "claude-code",
        label: "Claude Code",
        account: "Tu suscripción de Claude (Pro o Max) o una cuenta de Claude Platform",
        bins: &["claude"],
        install: Some("curl -fsSL https://claude.ai/install.sh | bash"),
        url: "https://docs.claude.com/claude-code",
        login: "claude auth login",
        login_note: "Se abre el navegador para entrar con tu cuenta de Claude.",
        session: Session::Ask(&["auth", "status", "--json"], claude_logged_in),
        // Claude Code nunca corre en el encierro de los demás: su carpeta no se les abre.
        dirs: &[],
        legacy: &[],
        free: None,
    },
    Cli {
        id: "codex",
        label: "Codex",
        account: "Tu cuenta de ChatGPT (Free, Plus, Pro, Business) o una llave de OpenAI",
        bins: &["codex"],
        install: Some("npm install -g @openai/codex"),
        url: "https://github.com/openai/codex",
        login: "codex login",
        login_note: "Se abre el navegador para entrar con tu cuenta de ChatGPT.",
        session: Session::Ask(&["login", "status"], exit_ok),
        dirs: &[".codex"],
        legacy: &[],
        free: Some("gratis con ChatGPT Free (con límite de uso)"),
    },
    Cli {
        id: "antigravity",
        label: "Antigravity CLI",
        account: "Tu cuenta de Google (gratis, Google AI Pro o Ultra)",
        bins: &["agy"],
        install: Some("curl -fsSL https://antigravity.google/cli/install.sh | bash"),
        url: "https://antigravity.google/docs/cli/",
        login: "agy",
        login_note: "Elige «Google OAuth» y entra en el navegador. Cuando aparezca el cuadro para escribir, ciérralo con Ctrl+C.",
        // `agy models` lista los modelos con la sesión iniciada y termina con error sin ella.
        session: Session::Ask(&["models"], exit_ok),
        dirs: &[".gemini/antigravity-cli"],
        legacy: &[],
        free: Some("gratis con tu cuenta personal de Google (cuota semanal)"),
    },
    Cli {
        id: "gemini-cli",
        label: "Gemini CLI",
        // Desde el 18 de junio de 2026 Google no atiende aquí cuentas personales (gratis,
        // AI Pro ni Ultra): las manda a Antigravity.
        account: "Una llave de Gemini, Vertex AI o una cuenta de empresa. Con tu cuenta personal de Google, usa Antigravity CLI",
        bins: &["gemini"],
        install: Some("npm install -g @google/gemini-cli"),
        url: "https://github.com/google-gemini/gemini-cli",
        login: "gemini",
        login_note: "Google ya no acepta cuentas personales en Gemini CLI: elige «Use Gemini API Key» o «Vertex AI», o usa Antigravity CLI. Al terminar, escribe /quit. Si usas llave, pégala también en su conexión (Conexiones): en modo automático Gemini CLI solo la lee de ahí.",
        // Su archivo de sesión queda aunque Google rechace la cuenta: no sirve para saberlo.
        session: Session::Unknown,
        dirs: &[".gemini"],
        legacy: &[],
        free: Some("gratis con una llave de Google AI Studio (capa gratis)"),
    },
    Cli {
        id: "qwen-code",
        label: "Qwen Code",
        account: "Un modelo de tu Mac, una llave o tu suscripción de Qwen",
        bins: &["qwen"],
        install: Some("npm install -g @qwen-code/qwen-code"),
        url: "https://github.com/QwenLM/qwen-code",
        login: "qwen",
        // El plan gratis de Qwen OAuth se cerró el 15 de abril de 2026.
        login_note: "Con cuenta, Qwen pide suscripción (Plus o Pro): elige «Qwen Account», entra en el navegador y luego escribe /quit. Gratis: en Atalaya elige un modelo de tu Mac o de un servicio, sin iniciar sesión.",
        session: Session::Files(&[".qwen/oauth_creds.json"]),
        dirs: &[".qwen"],
        legacy: &[],
        free: Some("gratis con un modelo de tu Mac o una llave gratis"),
    },
    Cli {
        id: "kimi",
        label: "Kimi Code",
        account: "Tu cuenta de Kimi (Moonshot)",
        bins: &["kimi"],
        install: Some("curl -fsSL https://code.kimi.com/kimi-code/install.sh | bash"),
        url: "https://www.kimi.com/code/docs/en/kimi-code-cli/",
        login: "kimi login",
        login_note: "Abre el enlace que aparece, escribe el código y autoriza tu cuenta de Kimi.",
        // Sus credenciales viven en ~/.kimi-code/credentials (la subcarpeta mcp es de otros servidores).
        session: Session::Folder(".kimi-code/credentials", &["mcp"]),
        dirs: &[".kimi-code", ".kimi"],
        // El kimi-cli de Python (instalado con uv) quedó sin mantenimiento.
        legacy: &[("/uv/tools/kimi-cli/", "kimi-cli")],
        free: None,
    },
    Cli {
        id: "copilot",
        label: "GitHub Copilot",
        account: "Tu cuenta de GitHub Copilot (Free o de pago)",
        bins: &["copilot"],
        install: Some("npm install -g @github/copilot"),
        url: "https://github.com/github/copilot-cli",
        // `copilot login` guarda su propia sesión. Sin ella, Copilot usa la llave de `gh`,
        // que suele no tener el permiso «Copilot Requests».
        login: "copilot login",
        login_note: "Se abre el navegador para entrar con tu cuenta de GitHub. Así Copilot deja de usar la llave de gh.",
        session: Session::Unknown,
        dirs: &[".copilot"],
        legacy: &[],
        free: Some("gratis con Copilot Free (modelo automático, con límites)"),
    },
    Cli {
        id: "cursor",
        label: "Cursor",
        account: "Tu cuenta de Cursor (Hobby gratis o de pago)",
        bins: &["cursor-agent"],
        install: Some("curl https://cursor.com/install -fsS | bash"),
        url: "https://cursor.com/cli",
        login: "cursor-agent login",
        login_note: "Se abre el navegador para entrar con tu cuenta de Cursor.",
        session: Session::Ask(&["status"], says_logged_in),
        dirs: &[".cursor", ".config/cursor"],
        legacy: &[],
        free: Some("gratis con el plan Hobby de Cursor (pedidos de agente limitados)"),
    },
    Cli {
        id: "opencode",
        label: "OpenCode",
        account: "Muchos proveedores: Claude, ChatGPT, Copilot, OpenRouter, modelos locales…",
        bins: &["opencode"],
        install: Some("npm install -g opencode-ai"),
        url: "https://opencode.ai",
        login: "opencode auth login",
        login_note: "Elige tu proveedor y sigue los pasos.",
        // `opencode auth list` cuenta las credenciales que tiene (sin mostrarlas).
        session: Session::Ask(&["auth", "list"], opencode_has_credentials),
        dirs: &[".config/opencode", ".local/share/opencode", ".opencode"],
        legacy: &[],
        free: Some("trae modelos gratis de OpenCode Zen (cambian seguido)"),
    },
    Cli {
        id: "droid",
        label: "Factory Droid",
        account: "Tu cuenta de Factory",
        bins: &["droid"],
        install: Some("curl -fsSL https://app.factory.ai/cli | sh"),
        url: "https://factory.ai",
        login: "droid",
        login_note: "Entra con tu cuenta de Factory y luego cierra con /quit.",
        // `droid doctor --checks auth` revisa la sesión guardada sin gastar nada.
        session: Session::Ask(&["doctor", "--checks", "auth"], exit_ok),
        dirs: &[".factory"],
        legacy: &[],
        free: None,
    },
    Cli {
        id: "amp",
        label: "Amp",
        account: "Tu cuenta de Amp",
        bins: &["amp"],
        install: Some("npm install -g @sourcegraph/amp"),
        url: "https://ampcode.com",
        login: "amp login",
        login_note: "Se abre el navegador para entrar con tu cuenta de Amp.",
        // `amp account list` marca la cuenta en uso con su id de usuario.
        session: Session::Ask(&["account", "list"], amp_has_account),
        dirs: &[".config/amp", ".local/share/amp", ".amp"],
        legacy: &[],
        free: None,
    },
];

pub fn get(id: &str) -> Option<&'static Cli> {
    CATALOG.iter().find(|c| c.id == id)
}

/// Dónde está instalado (sin contar versiones antiguas que ya no funcionan).
pub fn find(cli: &Cli) -> Option<PathBuf> {
    cli.bins.iter().find_map(|b| crate::agents::find_program(b))
}

/// Si este programa es una versión antigua conocida que ya no funciona, su nombre.
pub fn legacy_of(program: &Path) -> Option<&'static str> {
    let real = std::fs::canonicalize(program).unwrap_or_else(|_| program.to_path_buf());
    let real = real.to_string_lossy();
    CATALOG.iter().flat_map(|c| c.legacy.iter()).find(|(part, _)| real.contains(part)).map(|(_, name)| *name)
}

/// Una versión antigua instalada en lugar de la que sirve: (nombre, ruta).
fn legacy_installed(cli: &Cli) -> Option<(&'static str, PathBuf)> {
    if cli.legacy.is_empty() {
        return None;
    }
    let home = std::env::var("HOME").unwrap_or_default();
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    dirs.extend(["/opt/homebrew/bin".into(), "/usr/local/bin".into(), PathBuf::from(format!("{home}/.local/bin"))]);
    cli.bins
        .iter()
        .flat_map(|b| dirs.iter().map(move |d| d.join(b)))
        .filter(|p| p.is_file())
        .find_map(|p| legacy_of(&p).map(|name| (name, p)))
}

/// Limpia la salida de un agente para leerla como texto: Kimi Code marca cada párrafo
/// con «• » y sangra con dos espacios las líneas que siguen.
pub fn clean_line<'a>(provider: &str, line: &'a str) -> &'a str {
    if provider != "kimi" {
        return line;
    }
    line.strip_prefix("• ").or_else(|| line.strip_prefix("  ")).unwrap_or(line)
}

pub fn clean_output(provider: &str, text: &str) -> String {
    if provider != "kimi" {
        return text.to_string();
    }
    text.lines().map(|l| clean_line(provider, l)).collect::<Vec<_>>().join("\n")
}

/// Ejecuta un programa con un tiempo límite. Devuelve (terminó bien, salida y errores).
fn run_quick(program: &Path, args: &[&str], timeout: Duration) -> Option<(bool, String)> {
    let mut child = Command::new(program)
        .args(args)
        .env("PATH", crate::worker::developer_path())
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let start = Instant::now();
    loop {
        if let Ok(Some(_)) = child.try_wait() {
            break;
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let out = child.wait_with_output().ok()?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    Some((out.status.success(), text))
}

/// `lista`, `sin-sesion` o `desconocida`.
fn session_of(cli: &Cli, path: &Path) -> &'static str {
    match cli.session {
        Session::Ask(args, read) => match run_quick(path, args, Duration::from_secs(8)) {
            Some((ok, out)) if read(ok, &out) => "lista",
            Some(_) => "sin-sesion",
            None => "desconocida",
        },
        Session::Files(files) => {
            let home = dirs::home_dir().unwrap_or_default();
            if files.iter().any(|f| home.join(f).exists()) { "lista" } else { "sin-sesion" }
        }
        Session::Folder(folder, skip) => {
            let home = dirs::home_dir().unwrap_or_default();
            let found = std::fs::read_dir(home.join(folder)).is_ok_and(|entries| {
                entries.flatten().any(|e| e.file_name().to_str().is_some_and(|n| !n.starts_with('.') && !skip.contains(&n)))
            });
            if found { "lista" } else { "sin-sesion" }
        }
        Session::Unknown => "desconocida",
    }
}

/// Lo que se muestra de cada agente.
#[derive(Debug, Clone, Serialize)]
pub struct State {
    #[serde(flatten)]
    pub cli: Cli,
    pub installed: bool,
    pub path: Option<String>,
    pub version: Option<String>,
    /// `lista`, `sin-sesion`, `desconocida` o `no-instalado`.
    pub session: &'static str,
    /// La conexión que ya lo usa en Atalaya.
    pub connection: Option<String>,
    /// Hay una versión antigua instalada que ya no funciona (su nombre), y falta la nueva.
    pub legacy: Option<String>,
    /// Puede usar un modelo de este Mac o de un servicio en vez de su cuenta.
    pub own_model: bool,
    /// El modelo propio que usa ahora («qwen3.5:4b · http://…»), si usa uno.
    pub model_source: Option<String>,
    /// Si ese modelo propio es gratis.
    pub model_free: bool,
}

/// Las respuestas recientes, para no preguntar a cada programa en cada vista.
static CACHE: Mutex<Option<HashMap<&'static str, (Instant, &'static str, Option<String>)>>> = Mutex::new(None);

/// Olvida lo que se sabía de un agente (después de iniciar sesión o instalarlo).
pub fn forget(id: &str) {
    if let Some(map) = CACHE.lock().unwrap().as_mut() {
        map.retain(|k, _| *k != id);
    }
}

/// (sesión, versión) de un agente instalado, con 60 segundos de memoria.
fn probe(cli: &'static Cli, path: &Path) -> (&'static str, Option<String>) {
    if let Some((at, session, version)) = CACHE.lock().unwrap().as_ref().and_then(|m| m.get(cli.id)).cloned() {
        if at.elapsed() < Duration::from_secs(60) {
            return (session, version);
        }
    }
    let version = run_quick(path, &["--version"], Duration::from_secs(8))
        .and_then(|(_, out)| out.lines().map(str::trim).find(|l| !l.is_empty()).map(String::from));
    let session = session_of(cli, path);
    CACHE.lock().unwrap().get_or_insert_with(HashMap::new).insert(cli.id, (Instant::now(), session, version.clone()));
    (session, version)
}

pub fn state(registry: &Registry, cli: &'static Cli) -> Result<State> {
    let path = find(cli);
    let (session, version) = match &path {
        Some(p) => probe(cli, p),
        None => ("no-instalado", None),
    };
    // Claude Code instalado ya está en Atalaya sin conexión guardada: es el de siempre.
    let saved = crate::connections::of_provider(registry, cli.id)?.into_iter().next();
    let connection = if cli.id == "claude-code" && path.is_some() {
        Some(crate::orchestrator::CLAUDE_CODE.to_string())
    } else {
        saved.as_ref().map(|c| c.id.clone())
    };
    let own = saved.as_ref().and_then(crate::agents::own_model);
    // Con un modelo propio no necesita la sesión de su cuenta.
    let session = if own.is_some() && path.is_some() { "lista" } else { session };
    let legacy = if path.is_none() { legacy_installed(cli).map(|(name, _)| name.to_string()) } else { None };
    Ok(State {
        legacy,
        cli: cli.clone(),
        installed: path.is_some(),
        path: path.map(|p| p.display().to_string()),
        version,
        session,
        connection,
        own_model: crate::agents::supports_own_model(cli.id),
        model_free: own.as_ref().is_some_and(|(base, model)| crate::connections::is_free_endpoint(base, model)),
        model_source: own.map(|(base, model)| format!("{model} · {base}")),
    })
}

/// Todos los agentes del catálogo, en paralelo (cada uno puede tardar unos segundos).
pub fn states(registry: &Registry) -> Result<Vec<State>> {
    let path = registry.path();
    let handles: Vec<_> = CATALOG
        .iter()
        .map(|cli| {
            let path = path.clone();
            std::thread::spawn(move || -> Result<State> { state(&Registry::open(&path)?, cli) })
        })
        .collect();
    handles.into_iter().map(|h| h.join().map_err(|_| anyhow::anyhow!("falló la revisión de un agente"))?).collect()
}

/// Por qué un agente no puede usarse ahora, o `None` si puede (o no se sabe).
pub fn unavailable(c: &Connection) -> Option<String> {
    if crate::agents::own_model(c).is_some() {
        return None;
    }
    let cli = get(&c.provider)?;
    let path = find(cli)?;
    (probe(cli, &path).0 == "sin-sesion").then(|| format!("falta iniciar sesión en {}", cli.label))
}

/// Abre la Terminal con un comando, para que la persona lo vea y lo termine.
/// Usa un archivo `.command`, que la Terminal abre sola: no hace falta permiso para controlarla.
fn open_in_terminal(name: &str, title: &str, command: &str, note: &str) -> Result<()> {
    let dir = std::env::temp_dir().join("atalaya-terminal");
    std::fs::create_dir_all(&dir)?;
    let file = dir.join(format!("{name}.command"));
    let script = format!(
        "#!/bin/zsh -l\nexport PATH=\"{path}:$PATH\"\nclear\necho \"Atalaya · {title}\"\necho \"{note}\"\necho\n{command}\necho\necho \"Listo. Vuelve a Atalaya y pulsa Comprobar; puedes cerrar esta ventana.\"\n",
        path = crate::worker::developer_path(),
    );
    std::fs::write(&file, script)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o700))?;
    }
    let status = Command::new("/usr/bin/open").args(["-a", "Terminal"]).arg(&file).status()?;
    if !status.success() {
        bail!("no se pudo abrir la Terminal");
    }
    Ok(())
}

/// Abre la Terminal con el inicio de sesión del agente.
pub fn login(id: &str) -> Result<()> {
    let cli = get(id).with_context(|| format!("agente desconocido '{id}'"))?;
    if find(cli).is_none() {
        bail!("{} no está instalado", cli.label);
    }
    forget(id);
    open_in_terminal(&format!("sesion-{id}"), &format!("Iniciar sesión en {}", cli.label), cli.login, cli.login_note)
}

/// Abre la Terminal con la instalación del agente.
pub fn install(id: &str) -> Result<()> {
    let cli = get(id).with_context(|| format!("agente desconocido '{id}'"))?;
    let command = cli.install.with_context(|| format!("{} se instala desde {}", cli.label, cli.url))?;
    if command.starts_with("npm ") && crate::agents::find_program("npm").is_none() {
        bail!("para instalar {} hace falta Node.js (npm); instálalo desde https://nodejs.org", cli.label);
    }
    if command.starts_with("uv ") && crate::agents::find_program("uv").is_none() {
        bail!("para instalar {} hace falta uv; instálalo desde https://docs.astral.sh/uv/", cli.label);
    }
    forget(id);
    open_in_terminal(&format!("instalar-{id}"), &format!("Instalar {}", cli.label), command, "Puede pedirte confirmar algo.")
}

/// Crea la conexión del agente si no existe y devuelve su id.
pub fn connect(registry: &Registry, id: &str) -> Result<String> {
    let cli = get(id).with_context(|| format!("agente desconocido '{id}'"))?;
    if let Some(c) = crate::connections::of_provider(registry, id)?.into_iter().next() {
        return Ok(c.id);
    }
    let c = crate::connections::save(
        registry,
        &crate::connections::Input {
            id: None,
            provider: id.into(),
            name: cli.label.into(),
            config: serde_json::json!({}),
            secret: None,
        },
    )?;
    Ok(c.id)
}

/// Un modelo que un agente puede usar en vez de su cuenta.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelSource {
    /// Cómo se muestra: «Ollama · qwen3.5:4b».
    pub label: String,
    pub base_url: String,
    pub model: String,
    /// Cuenta del Llavero con la llave del servicio (nunca la llave misma).
    pub key_account: Option<String>,
    /// `local` (este Mac) o `nube`.
    pub place: String,
    pub free: bool,
}

/// Los modelos que los agentes pueden usar con la API de OpenAI: los de este Mac que estén
/// sirviendo y las conexiones de modelos guardadas. Los gratis primero.
pub fn model_sources(registry: &Registry) -> Result<Vec<ModelSource>> {
    let mut out = Vec::new();
    for server in crate::local::detect() {
        for model in &server.models {
            // Los modelos «:cloud» de Ollama corren en sus servidores y se cobran por uso.
            let cloud = crate::connections::is_cloud_model(model);
            out.push(ModelSource {
                label: format!("{} · {model}{}", server.label, if cloud { " (nube de Ollama, de pago)" } else { "" }),
                base_url: server.base_url.to_string(),
                model: model.clone(),
                key_account: None,
                place: if cloud { "nube" } else { "local" }.into(),
                free: !cloud,
            });
        }
    }
    for c in crate::connections::all(registry)? {
        let base = match c.provider.as_str() {
            "openai" => Some("https://api.openai.com/v1".to_string()),
            "openai-compatible" | "ollama" => c.config_str("base_url"),
            _ => None,
        };
        let (Some(base), Some(model)) = (base.map(|b| crate::local::loopback(&b)), c.config_str("model")) else { continue };
        if out.iter().any(|s: &ModelSource| s.base_url.trim_end_matches('/') == base.trim_end_matches('/') && s.model == model) {
            continue;
        }
        let local = base.contains("://127.0.0.1");
        // Uno de este Mac sirve solo si su servidor lo tiene: así no se elige un nombre mal escrito.
        if local && !crate::local::models(&base).is_some_and(|list| list.contains(&model)) {
            continue;
        }
        let free = crate::connections::is_free_endpoint(&base, &model);
        out.push(ModelSource {
            label: format!("{} · {model}", c.name),
            base_url: base,
            model,
            key_account: c.secret_account.clone(),
            place: if local { "local" } else { "nube" }.into(),
            free,
        });
    }
    // Estable: gratis primero; entre ellos, primero lo de este Mac.
    out.sort_by_key(|s| (!s.free, s.place != "local"));
    Ok(out)
}

/// Elige el modelo propio de un agente (`None` vuelve a su cuenta). Crea su conexión si falta.
pub fn set_model(registry: &Registry, id: &str, source: Option<&ModelSource>) -> Result<String> {
    if !crate::agents::supports_own_model(id) {
        bail!("{id} no puede usar otro modelo; usa su cuenta");
    }
    let conn_id = connect(registry, id)?;
    let c = crate::connections::get(registry, &conn_id)?.context("no se encontró la conexión del agente")?;
    let mut config = c.config.clone();
    if !config.is_object() {
        config = serde_json::json!({});
    }
    let entries = [
        ("model_base_url", source.map(|s| s.base_url.clone())),
        ("model", source.map(|s| s.model.clone())),
        ("model_key", source.and_then(|s| s.key_account.clone())),
    ];
    for (key, value) in entries {
        match value {
            Some(v) => config[key] = serde_json::json!(v),
            None => {
                config.as_object_mut().map(|o| o.remove(key));
            }
        }
    }
    crate::connections::save(
        registry,
        &crate::connections::Input { id: Some(c.id.clone()), provider: c.provider.clone(), name: c.name.clone(), config, secret: None },
    )?;
    forget(id);
    Ok(conn_id)
}

/// Las carpetas de los proyectos registrados: quien solo responde no las puede cambiar.
pub fn project_folders(registry: &Registry) -> Vec<PathBuf> {
    registry
        .list()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|p| p.local_path.map(PathBuf::from))
        .filter(|p| p.is_dir())
        .collect()
}

/// Tiempo máximo de una respuesta de texto.
pub const ASK_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// Una pregunta de texto a un agente, en solo lectura: dentro del encierro solo escribe
/// en una carpeta temporal y en su propia configuración.
pub fn ask(registry: &Registry, c: &Connection, system: &str, prompt: &str, task: &str, slug: Option<&str>) -> Result<String> {
    let result = run_ask(registry, c, &format!("{system}\n\n{prompt}"), ASK_TIMEOUT);
    let model = c.config_str("model").unwrap_or_else(|| c.provider.clone());
    match &result {
        Ok(r) => crate::ai::record(
            registry,
            c,
            &model,
            task,
            slug,
            (r.input_tokens, r.output_tokens),
            None,
            (!r.ok).then(|| crate::quota::agent_error(&r.output, r.timed_out)).as_deref(),
        )?,
        Err(e) => crate::ai::record(registry, c, &model, task, slug, (None, None), None, Some(&format!("{e:#}")))?,
    }
    answer_of(c, result?)
}

/// Lanza el agente encerrado y en solo lectura con un texto ya armado.
fn run_ask(registry: &Registry, c: &Connection, full: &str, timeout: Duration) -> Result<crate::agents::AgentRun> {
    let template = crate::agents::ask_template(c)?;
    let scratch = std::env::temp_dir().join(format!("atalaya-pregunta-{}", std::process::id()));
    std::fs::create_dir_all(&scratch)?;
    let mut policy = crate::agents::Policy::for_workdir(&scratch);
    policy.env = crate::agents::key_env(c);
    policy.protected = project_folders(registry);
    crate::mcp_servers::fill(registry, None, &c.id, &mut policy)?;
    crate::agents::run_template(&template, &scratch, full, &policy, timeout)
}

/// La respuesta del agente, o un error que se entienda.
fn answer_of(c: &Connection, r: crate::agents::AgentRun) -> Result<String> {
    if r.timed_out {
        bail!("{} no respondió a tiempo", c.name);
    }
    let text = clean_output(&c.provider, r.stdout.trim()).trim().to_string();
    if !r.ok || text.is_empty() {
        bail!("{} no respondió: {}", c.name, explain_failure(&r.output));
    }
    Ok(text)
}

/// Lo que importa de una salida con error: el `detail` o `message` de un JSON si lo trae,
/// o las últimas líneas que no son avisos.
pub fn explain_failure(output: &str) -> String {
    let detail = failure_detail(output);
    let lower = output.to_lowercase();
    // Sin saldo: el servicio cobra y la cuenta no tiene créditos ni plan.
    if ["402", "usage credits", "insufficient balance", "insufficient_quota", "payment required"].iter().any(|k| lower.contains(k)) {
        return format!("ese modelo se cobra y tu cuenta no tiene saldo ni plan; elige uno gratis o agrega créditos ({detail})");
    }
    detail
}

fn failure_detail(output: &str) -> String {
    for key in ["\"detail\":\"", "\"message\":\""] {
        if let Some(start) = output.find(key) {
            let rest = &output[start + key.len()..];
            let mut text = String::new();
            let mut chars = rest.chars();
            while let Some(ch) = chars.next() {
                match ch {
                    '"' => break,
                    '\\' => match chars.next() {
                        Some('n') => text.push(' '),
                        Some(other) => text.push(other),
                        None => break,
                    },
                    _ => text.push(ch),
                }
            }
            if !text.trim().is_empty() {
                return text.trim().to_string();
            }
        }
    }
    let lines: Vec<&str> = output
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.to_lowercase().starts_with("warning"))
        .collect();
    let tail = lines[lines.len().saturating_sub(3)..].join(" ");
    let tail: String = tail.chars().rev().take(400).collect::<Vec<_>>().into_iter().rev().collect();
    if tail.is_empty() { "sin salida".into() } else { tail }
}

/// Tiempo máximo de la pregunta de prueba.
pub const VERIFY_TIMEOUT: Duration = Duration::from_secs(150);

/// Valida un agente de verdad: que esté instalado, que tenga la sesión y que responda una
/// pregunta corta dentro de su encierro, en solo lectura. Devuelve lo que contestó.
/// Gasta muy poco de la cuenta del agente.
pub fn verify(registry: &Registry, id: &str) -> Result<String> {
    let cli = get(id).with_context(|| format!("agente desconocido '{id}'"))?;
    let path = find(cli).with_context(|| match legacy_installed(cli) {
        Some((old, _)) => format!("tienes {old}, que ya no funciona: instala {}", cli.label),
        None => format!("{} no está instalado", cli.label),
    })?;
    forget(id);
    let saved = crate::connections::of_provider(registry, id)?.into_iter().next();
    let own = saved.as_ref().is_some_and(|c| crate::agents::own_model(c).is_some());
    let (session, _) = probe(cli, &path);
    if session == "sin-sesion" && !own {
        bail!("falta iniciar sesión en {}. {}", cli.label, cli.login_note);
    }
    // Claude Code no corre en este encierro (lo lanza su propio flujo); su estado es fiable.
    if cli.id == "claude-code" {
        return Ok("sesión iniciada".into());
    }
    let c = saved.unwrap_or_else(|| Connection {
        id: format!("prueba-{id}"),
        kind: "agente".into(),
        provider: id.into(),
        name: cli.label.into(),
        config: serde_json::json!({}),
        secret_account: None,
        created_at: String::new(),
    });
    let r = run_ask(registry, &c, "Responde solo con la palabra: listo", VERIFY_TIMEOUT)?;
    let text = answer_of(&c, r)?;
    let first: String = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or_default().chars().take(80).collect();
    Ok(format!("respondió «{first}»"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_agent_has_a_connection_provider_and_reads_sessions() {
        for cli in CATALOG {
            let p = crate::connections::provider(cli.id).unwrap_or_else(|| panic!("falta el proveedor {}", cli.id));
            assert_eq!(p.kind, "agente");
            assert!(!cli.bins.is_empty() && !cli.login.is_empty());
        }
        assert!(claude_logged_in(true, r#"{"loggedIn": true}"#));
        assert!(!claude_logged_in(true, r#"{"loggedIn": false}"#));
        assert!(says_logged_in(true, "✓ Logged in as alguien"));
        assert!(!says_logged_in(true, "Not logged in"));
        assert!(amp_has_account(true, "* alguien (user_01ABC)"));
        assert!(!amp_has_account(true, "No saved accounts"));
        assert!(opencode_has_credentials(true, "●  Google api\n└  1 credentials"));
        assert!(!opencode_has_credentials(true, "└  0 credentials"));
    }

    #[test]
    fn explains_failures_in_plain_words() {
        let droid = r#"Error during droid execution: 402 {"detail":"No active subscription found.\nAdd your own model key.","status":402}"#;
        assert_eq!(
            explain_failure(droid),
            "ese modelo se cobra y tu cuenta no tiene saldo ni plan; elige uno gratis o agrega créditos (No active subscription found. Add your own model key.)"
        );
        let ollama = "Qwen Code no respondió: [API Error: 402 Add usage credits or subscribe: https://ollama.com/plans]";
        assert!(explain_failure(ollama).starts_with("ese modelo se cobra"), "{}", explain_failure(ollama));
        assert_eq!(explain_failure("Warning: yolo\nNo auth type is selected."), "No auth type is selected.");
    }

    /// El kimi-cli antiguo (instalado con uv) no cuenta como Kimi Code, aunque se llame igual.
    #[test]
    fn ignores_retired_versions_with_the_same_name() {
        let base = std::env::temp_dir().join(format!("atalaya-legado-{}", std::process::id()));
        let old = base.join("uv/tools/kimi-cli/bin");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("kimi"), "").unwrap();
        std::fs::write(base.join("kimi"), "").unwrap();
        assert_eq!(legacy_of(&old.join("kimi")), Some("kimi-cli"));
        assert_eq!(legacy_of(&base.join("kimi")), None);
        assert_eq!(crate::agents::find_program(&old.join("kimi").display().to_string()), Some(old.join("kimi")));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn cleans_kimi_transcript_marks() {
        assert_eq!(clean_output("kimi", "• Hola\n  segunda línea\n• Otra"), "Hola\nsegunda línea\nOtra");
        assert_eq!(clean_output("codex", "• queda igual"), "• queda igual");
    }

    /// Un agente falso responde texto; la respuesta es su salida normal y queda anotado el consumo.
    #[test]
    fn asks_a_terminal_agent_for_text() {
        let r = crate::testutil::registry("clis-preguntar");
        let script = std::env::temp_dir().join(format!("atalaya-cli-falso-{}.sh", std::process::id()));
        std::fs::write(&script, "echo \"respuesta a: $1\" | head -c 60\necho ruido >&2\n").unwrap();
        let c = crate::connections::save(
            &r,
            &crate::connections::Input {
                id: None,
                provider: "agente-personalizado".into(),
                name: "Falso".into(),
                config: serde_json::json!({ "command": format!("/bin/sh {} {{tarea}}", script.display()) }),
                secret: None,
            },
        )
        .unwrap();
        let text = ask(&r, &c, "sistema", "hola", "prueba", None).unwrap();
        assert!(text.starts_with("respuesta a: sistema"), "{text}");
        assert!(!text.contains("ruido"));
        let _ = std::fs::remove_file(&script);
    }
}
