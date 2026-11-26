//! Agentes de terminal que hacen cambios de código: Claude Code, Codex, Antigravity,
//! Kimi Code, Cursor, OpenCode o uno propio con plantilla de comando. Algunos (como Qwen
//! Code) pueden usar un modelo de este Mac o de un servicio en vez de su cuenta: `OWN_MODELS`.
//!
//! Claude Code trae su propio aislamiento y lo maneja `worker`. Los demás corren aquí
//! dentro de un perfil de `sandbox-exec` generado en cada ejecución:
//! - solo escriben en su copia de trabajo, en las carpetas temporales y en sus cachés;
//! - no leen `~/.ssh`, `~/.aws`, `~/.config/gh`, `~/.gnupg` ni los datos de Atalaya;
//! - pueden pedir al Llavero su propia sesión (y macOS lo usa para validar HTTPS): macOS
//!   solo entrega a cada programa lo que guardó él y pregunta por lo demás;
//! - sin credenciales no pueden subir código: subir la rama y abrir el PR lo hace Atalaya.

use crate::Registry;
use crate::connections::Connection;
use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde_json::Value;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// La plantilla para trabajar (puede cambiar archivos).
pub fn template(c: &Connection) -> Result<String> {
    Ok(match c.provider.as_str() {
        "claude-code" => "claude".to_string(),
        _ => with_own_model(c, c.config_str("command").context("el agente no tiene comando")?),
    })
}

/// La plantilla para responder; si no tiene una propia, la de trabajar.
pub fn ask_template(c: &Connection) -> Result<String> {
    match c.config_str("ask_command") {
        Some(t) => Ok(with_own_model(c, t)),
        None => template(c),
    }
}

/// Un agente que puede usar otro modelo en vez de su cuenta: uno de este Mac (Ollama,
/// LM Studio…) o de un servicio con la API de OpenAI. Así un agente de pago sale gratis.
pub struct OwnModel {
    pub provider: &'static str,
    /// Se agregan después del programa cuando usa un modelo propio.
    args: &'static [&'static str],
    /// Las variables donde lee la dirección, el modelo y la llave.
    base_env: &'static str,
    model_env: &'static str,
    key_env: &'static str,
}

pub const OWN_MODELS: &[OwnModel] = &[OwnModel {
    provider: "qwen-code",
    // --bare: sin las skills, MCP ni contexto implícito de la persona. Con un modelo local
    // eso baja cada pedido de unos 28 000 tokens a 7000 (de minutos a segundos).
    args: &["--bare", "--auth-type", "openai"],
    base_env: "OPENAI_BASE_URL",
    model_env: "OPENAI_MODEL",
    key_env: "OPENAI_API_KEY",
}];

/// Si este agente puede usar un modelo propio.
pub fn supports_own_model(provider: &str) -> bool {
    OWN_MODELS.iter().any(|m| m.provider == provider)
}

/// El modelo propio que usa la conexión: (dirección, modelo). `None` si usa su cuenta.
pub fn own_model(c: &Connection) -> Option<(String, String)> {
    if !supports_own_model(&c.provider) {
        return None;
    }
    Some((c.config_str("model_base_url")?, c.config_str("model")?))
}

fn with_own_model(c: &Connection, template: String) -> String {
    let Some(spec) = OWN_MODELS.iter().find(|m| m.provider == c.provider) else { return template };
    if own_model(c).is_none() {
        return template;
    }
    let mut words = template.split_whitespace();
    let program = words.next().unwrap_or_default();
    let rest: Vec<&str> = words.filter(|w| !spec.args.contains(w)).collect();
    format!("{program} {} {}", spec.args.join(" "), rest.join(" ")).trim().to_string()
}

/// El programa que lanza el agente (la primera palabra de su plantilla).
pub fn program(c: &Connection) -> Result<String> {
    let command = template(c)?;
    Ok(command.split_whitespace().next().context("el comando del agente está vacío")?.to_string())
}

/// Busca un programa en el PATH y en los lugares habituales de instalación.
pub fn find_program(name: &str) -> Option<PathBuf> {
    if name.contains('/') {
        let path = PathBuf::from(name);
        return path.is_file().then_some(path);
    }
    let home = std::env::var("HOME").unwrap_or_default();
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    for extra in [
        "/opt/homebrew/bin",
        "/usr/local/bin",
        &format!("{home}/.local/bin"),
        &format!("{home}/.npm-global/bin"),
        &format!("{home}/.bun/bin"),
        &format!("{home}/.cargo/bin"),
        &format!("{home}/.opencode/bin"),
        &format!("{home}/.kimi-code/bin"),
        &format!("{home}/.lmstudio/bin"),
    ] {
        dirs.push(PathBuf::from(extra));
    }
    // Las versiones antiguas que ya no funcionan (y comparten el nombre) no cuentan.
    dirs.into_iter().map(|d| d.join(name)).find(|p| p.is_file() && crate::clis::legacy_of(p).is_none())
}

/// Prueba un agente: que esté instalado y responda a `--version`.
pub fn test(c: &Connection) -> Result<String> {
    let name = program(c)?;
    let Some(path) = find_program(&name) else {
        bail!("no se encontró «{name}» instalado en este Mac");
    };
    // Fuera de cualquier proyecto: un agente personalizado podría no entender `--version` y
    // ponerse a trabajar donde esté parado.
    let out = Command::new(&path)
        .arg("--version")
        .current_dir(std::env::temp_dir())
        .output()
        .with_context(|| format!("no se pudo ejecutar {name}"))?;
    let version = String::from_utf8_lossy(if out.stdout.is_empty() { &out.stderr } else { &out.stdout })
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_string();
    if let Some(reason) = crate::clis::unavailable(c) {
        bail!("{reason}");
    }
    Ok(if version.is_empty() { format!("Instalado en {}", path.display()) } else { format!("Instalado: {version}") })
}

/// Arma los argumentos: la tarea entra entera en `{tarea}` (o al final), sin pasar por
/// una shell, así que no se puede inyectar nada con ella.
pub fn command_line(template: &str, task: &str) -> Vec<String> {
    let mut args: Vec<String> = template.split_whitespace().map(String::from).collect();
    let mut placed = false;
    for arg in &mut args {
        if arg.contains("{tarea}") {
            *arg = arg.replace("{tarea}", task);
            placed = true;
        }
    }
    if !placed {
        args.push(task.to_string());
    }
    args
}

/// Qué puede tocar un agente.
#[derive(Debug, Clone)]
pub struct Policy {
    /// Donde puede escribir.
    pub writable: Vec<PathBuf>,
    /// Lo que no puede ni leer ni escribir, aunque esté dentro de lo anterior.
    pub denied: Vec<PathBuf>,
    /// Lo que puede leer pero nunca escribir, aunque quede dentro de lo escribible (por
    /// ejemplo, las carpetas de los proyectos cuando el agente solo responde).
    pub protected: Vec<PathBuf>,
    /// Lo que puede leer aunque quede dentro de lo prohibido: los adjuntos del chat, que
    /// viven en la carpeta de datos de Atalaya.
    pub readable: Vec<PathBuf>,
    /// Variables que recibe además el agente: la llave de su propia conexión, si la tiene.
    pub env: Vec<(String, String)>,
    /// Servidores MCP externos que recibe además del de Atalaya (sus llaves van en `env`).
    pub mcp: Vec<crate::mcp_servers::Server>,
}

impl Policy {
    /// La política normal: su copia de trabajo, temporales, cachés y la configuración
    /// propia de cada agente; nada de llaves ni datos de Atalaya.
    pub fn for_workdir(workdir: &Path) -> Self {
        let home = dirs::home_dir().unwrap_or_default();
        let mut writable = vec![workdir.to_path_buf(), PathBuf::from("/private/tmp"), PathBuf::from("/private/var/folders")];
        for rel in [".cache", ".npm", ".bun", "Library/Caches", ".local/state", ".local/share/uv"] {
            writable.push(home.join(rel));
        }
        // Cada agente guarda y renueva ahí su sesión.
        for cli in crate::clis::CATALOG {
            for rel in cli.dirs {
                writable.push(home.join(rel));
            }
        }
        let mut denied: Vec<PathBuf> = [
            ".ssh",
            ".aws",
            ".config/gh",
            ".gnupg",
            ".git-credentials",
            ".netrc",
            ".docker/config.json",
        ]
        .iter()
        .map(|rel| home.join(rel))
        .collect();
        if let Some(data) = Registry::default_path().ok().and_then(|p| p.parent().map(Path::to_path_buf)) {
            denied.push(data);
        }
        // Los adjuntos del chat y las skills: solo para leer.
        let readable = crate::attachments::root().into_iter().chain(crate::skills::root()).collect();
        Self { writable, denied, protected: Vec::new(), readable, env: Vec::new(), mcp: Vec::new() }
    }

    /// Para una copia hecha con `git worktree`: además puede hacer commits en su rama.
    /// Los commits se guardan en el `.git` del repo original, así que se abre solo lo
    /// justo: los objetos, la rama de Atalaya y los datos de esta copia. Nunca los
    /// hooks ni la configuración, que correrían luego en la carpeta del usuario.
    pub fn for_worktree(workdir: &Path) -> Self {
        let mut policy = Self::for_workdir(workdir);
        policy.allow_worktree_commits(workdir);
        policy
    }

    fn allow_worktree_commits(&mut self, workdir: &Path) {
        let policy = self;
        let rev = |arg: &str| -> Option<PathBuf> {
            let out = Command::new("git").arg("-C").arg(workdir).args(["rev-parse", "--path-format=absolute", arg]).output().ok()?;
            out.status.success().then(|| PathBuf::from(String::from_utf8_lossy(&out.stdout).trim()))
        };
        if let (Some(common), Some(own)) = (rev("--git-common-dir"), rev("--git-dir")) {
            if own != common {
                policy.writable.push(own);
                for rel in ["objects", "refs/heads/atalaya", "logs/refs/heads/atalaya"] {
                    policy.writable.push(common.join(rel));
                }
            }
        }
    }
}

fn quote(path: &Path) -> String {
    let real = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    format!("\"{}\"", real.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\""))
}

/// El perfil de `sandbox-exec`. En estos perfiles manda la última regla que coincide.
pub fn profile(policy: &Policy) -> String {
    // Las terminales virtuales (PTY): los agentes ejecutan sus órdenes dentro de una y, sin
    // ellas, ninguna orden corre. Abrir una escribe en /dev/ptmx y en su /dev/ttysNNN.
    let mut p = String::from(
        "(version 1)\n(allow default)\n(deny file-write*)\n(allow file-write*\n  (literal \"/dev/null\") (literal \"/dev/tty\") (regex #\"^/dev/fd/\")\n  (literal \"/dev/ptmx\") (regex #\"^/dev/ttys[0-9]+$\")\n",
    );
    for dir in &policy.writable {
        p.push_str(&format!("  (subpath {})\n", quote(dir)));
    }
    p.push_str(")\n");
    if !policy.protected.is_empty() {
        p.push_str("(deny file-write*\n");
        for path in &policy.protected {
            p.push_str(&format!("  (subpath {})\n", quote(path)));
        }
        p.push_str(")\n");
    }
    if !policy.denied.is_empty() {
        p.push_str("(deny file-read* file-write*\n");
        for path in &policy.denied {
            p.push_str(&format!("  (subpath {})\n", quote(path)));
        }
        p.push_str(")\n");
    }
    // Va al final para ganarle a lo prohibido: solo leer, nunca escribir.
    if !policy.readable.is_empty() {
        p.push_str("(allow file-read*\n");
        for path in &policy.readable {
            p.push_str(&format!("  (subpath {})\n", quote(path)));
        }
        p.push_str(")\n");
    }
    p
}

/// El resultado de una ejecución.
#[derive(Debug, Clone, Serialize)]
pub struct AgentRun {
    pub ok: bool,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    /// Lo último que escribió el agente (salida y errores).
    pub output: String,
    /// Solo la salida normal: la respuesta, en los agentes que responden texto.
    pub stdout: String,
    /// Tokens, si el agente los informó en su salida. Si no, el consumo queda como no informado.
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
}

/// Busca en la salida (líneas JSON) el último conteo de tokens que informe el agente.
pub fn usage_from_output(text: &str) -> (Option<u64>, Option<u64>) {
    fn find(v: &Value, input: &mut Option<u64>, output: &mut Option<u64>) {
        match v {
            Value::Object(map) => {
                for (k, val) in map {
                    match (k.as_str(), val.as_u64()) {
                        ("input_tokens" | "prompt_tokens" | "promptTokenCount", Some(n)) => *input = Some(n),
                        ("output_tokens" | "completion_tokens" | "candidatesTokenCount", Some(n)) => *output = Some(n),
                        _ => find(val, input, output),
                    }
                }
            }
            Value::Array(items) => items.iter().for_each(|i| find(i, input, output)),
            _ => {}
        }
    }
    let (mut input, mut output) = (None, None);
    for line in text.lines() {
        if let Ok(v) = serde_json::from_str::<Value>(line.trim()) {
            find(&v, &mut input, &mut output);
        }
    }
    (input, output)
}

/// Variables de entorno que el agente hereda: lo justo para funcionar y las llaves de
/// los modelos, nunca las de GitHub ni las de Atalaya.
fn allowed_env(key: &str) -> bool {
    matches!(key, "PATH" | "HOME" | "USER" | "LOGNAME" | "LANG" | "LC_ALL" | "TERM" | "TMPDIR" | "SHELL")
        || [
            "OPENAI_", "GEMINI_", "GOOGLE_API", "GOOGLE_CLOUD_PROJECT", "ANTHROPIC_", "OPENROUTER_", "OLLAMA_", "MOONSHOT_", "KIMI_",
            "DASHSCOPE_", "QWEN_", "CURSOR_", "FACTORY_", "AMP_", "COPILOT_", "AGY_",
        ]
        .iter()
        .any(|p| key.starts_with(p))
}

/// Un agente en marcha. Mientras vive, su MCP de Atalaya (ver `mcp_bridge`) sigue abierto.
pub struct Running {
    pub child: std::process::Child,
    _mcp: Option<crate::mcp_bridge::Bridge>,
}

impl std::ops::Deref for Running {
    type Target = std::process::Child;
    fn deref(&self) -> &Self::Target {
        &self.child
    }
}

impl std::ops::DerefMut for Running {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.child
    }
}

/// Lanza un agente encerrado en `policy`, en `cwd`, con la salida conectada. El proceso
/// tiene su propio grupo: matar `-pid` lo detiene a él y a todo lo que lanzó. Lleva el MCP
/// de Atalaya en solo lectura cuando la CLI está instalada (o falla si la plantilla lo pide
/// con `{mcp_config}` y no se puede).
pub fn spawn(template: &str, cwd: &Path, task: &str, policy: &Policy) -> Result<Running> {
    let mut mcp = match crate::mcp_bridge::Bridge::start() {
        Ok(bridge) => Some(bridge),
        Err(e) if template.contains(crate::mcp_bridge::PLACEHOLDER) => return Err(e),
        Err(_) => None,
    };
    let mut args = command_line(template, task);
    let mut env = policy.env.clone();
    let mut policy = policy.clone();
    if let Some(bridge) = &mut mcp {
        bridge.apply(&mut args, &mut env, cwd, &policy.mcp);
        // Queda dentro de las carpetas temporales, donde el agente sí escribe.
        policy.protected.push(bridge.dir().to_path_buf());
    }
    let program = find_program(&args[0]).with_context(|| format!("no se encontró «{}» instalado", args[0]))?;
    let mut cmd = Command::new("/usr/bin/sandbox-exec");
    cmd.arg("-p").arg(profile(&policy)).arg(&program).args(&args[1..]);
    cmd.current_dir(cwd).env_clear().stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    for (k, v) in std::env::vars() {
        if allowed_env(&k) {
            cmd.env(k, v);
        }
    }
    for (k, v) in &env {
        cmd.env(k, v);
    }
    cmd.env("PATH", crate::worker::developer_path());
    // Sin colores ni animaciones: la salida se lee como texto.
    cmd.env("NO_COLOR", "1").env("TERM", "dumb").env("CI", "1");
    // Aunque encontrara credenciales, desde aquí no se puede subir al remoto.
    cmd.env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "remote.origin.pushurl")
        .env("GIT_CONFIG_VALUE_0", "bloqueado-por-atalaya://subir-requiere-tu-visto-bueno");
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    let child = cmd.spawn().context("no se pudo lanzar el agente")?;
    Ok(Running { child, _mcp: mcp })
}

/// Ejecuta un agente encerrado en `policy`, con un límite de tiempo.
pub fn run_with(c: &Connection, workdir: &Path, task: &str, policy: &Policy, timeout: Duration) -> Result<AgentRun> {
    if c.provider == "claude-code" {
        bail!("Claude Code trabaja con su propio aislamiento; lo lanza el flujo de mejoras");
    }
    let mut policy = policy.clone();
    policy.env.extend(key_env(c));
    run_template(&template(c)?, workdir, task, &policy, timeout)
}

/// Agentes que, en modo automático, solo leen su llave de una variable de entorno.
const KEY_ENV: &[(&str, &str)] = &[("gemini-cli", "GEMINI_API_KEY")];

/// Las variables que el agente necesita: su llave, y la dirección, el modelo y la llave del
/// modelo propio si usa uno. Solo las recibe ese agente; nunca van a la base ni a otros procesos.
pub fn key_env(c: &Connection) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = KEY_ENV
        .iter()
        .filter(|(provider, _)| *provider == c.provider)
        .filter_map(|(_, var)| c.secret().map(|key| (var.to_string(), key)))
        .collect();
    if let (Some(spec), Some((base, model))) = (OWN_MODELS.iter().find(|m| m.provider == c.provider), own_model(c)) {
        // Un servidor local no pide llave, pero el agente exige alguna.
        let key = c
            .config_str("model_key")
            .and_then(|account| crate::connections::secret_of_account(&account))
            .unwrap_or_else(|| "local".into());
        env.push((spec.base_env.into(), crate::local::loopback(&base)));
        env.push((spec.model_env.into(), model));
        env.push((spec.key_env.into(), key));
    }
    env
}

/// Recibe cada línea que escribe un agente mientras trabaja.
pub type Progress = std::sync::Arc<dyn Fn(&str) + Send + Sync>;

thread_local! {
    static PROGRESS: std::cell::RefCell<Option<Progress>> = const { std::cell::RefCell::new(None) };
}

/// Corre `f` avisando a `sink` cada línea de los agentes que lance en este hilo. Así el chat
/// muestra en vivo lo que va haciendo la IA a la que se le encargó algo.
pub fn with_progress<T>(sink: Option<Progress>, f: impl FnOnce() -> T) -> T {
    let previous = PROGRESS.with(|p| p.replace(sink));
    let out = f();
    PROGRESS.with(|p| *p.borrow_mut() = previous);
    out
}

/// Lee todo lo que escribe el agente, avisando cada línea a `sink` si hay.
fn read_all(mut from: impl Read + Send + 'static, sink: Option<Progress>) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let Some(sink) = sink else {
            let mut s = String::new();
            let _ = from.read_to_string(&mut s);
            return s;
        };
        let mut reader = std::io::BufReader::new(from);
        let mut all = Vec::new();
        let mut line = Vec::new();
        while std::io::BufRead::read_until(&mut reader, b'\n', &mut line).unwrap_or(0) > 0 {
            sink(&String::from_utf8_lossy(&line));
            all.append(&mut line);
        }
        String::from_utf8_lossy(&all).into_owned()
    })
}

/// Como `run_with`, con una plantilla cualquiera.
pub fn run_template(template: &str, workdir: &Path, task: &str, policy: &Policy, timeout: Duration) -> Result<AgentRun> {
    let mut child = spawn(template, workdir, task, policy)?;
    let group = child.id();
    // La salida se lee en hilos aparte para que el agente no se bloquee si escribe mucho.
    let stdout = child.stdout.take().context("sin salida")?;
    let stderr = child.stderr.take().context("sin salida de errores")?;
    let sink = PROGRESS.with(|p| p.borrow().clone());
    let out_reader = read_all(stdout, sink.clone());
    let err_reader = read_all(stderr, sink);
    let start = Instant::now();
    let mut timed_out = false;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break Some(status);
        }
        if start.elapsed() > timeout {
            let _ = Command::new("/bin/kill").args(["-9", &format!("-{group}")]).status();
            let _ = child.kill();
            timed_out = true;
            break child.wait().ok();
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    let out = out_reader.join().unwrap_or_default();
    let err = err_reader.join().unwrap_or_default();
    let (input_tokens, output_tokens) = usage_from_output(&out);
    let mut output = format!("{out}{}{err}", if err.is_empty() { "" } else { "\n" });
    if output.len() > 20_000 {
        let cut = output.char_indices().map(|(i, _)| i).find(|i| *i >= output.len() - 20_000).unwrap_or(0);
        output = format!("…{}", &output[cut..]);
    }
    Ok(AgentRun {
        ok: !timed_out && status.is_some_and(|s| s.success()),
        exit_code: status.and_then(|s| s.code()),
        timed_out,
        output,
        stdout: out,
        input_tokens,
        output_tokens,
    })
}

/// Ejecuta un agente en su copia de trabajo con la política normal y anota el consumo.
pub fn run(registry: &Registry, c: &Connection, workdir: &Path, task: &str, slug: Option<&str>, timeout: Duration) -> Result<AgentRun> {
    run_policy(registry, c, workdir, task, slug, timeout, &Policy::for_workdir(workdir), "agente")
}

/// Un trabajo que Claude Code le encarga a este agente desde el chat (ver `delegate`): escribe
/// solo en la carpeta del proyecto y queda anotado como «delegado».
pub fn run_delegated(registry: &Registry, c: &Connection, folder: &Path, task: &str, slug: Option<&str>, timeout: Duration) -> Result<AgentRun> {
    run_policy(registry, c, folder, task, slug, timeout, &Policy::for_workdir(folder), "delegado")
}

/// Como `run`, en una copia de `git worktree`: puede confirmar commits en su rama.
pub fn run_in_worktree(registry: &Registry, c: &Connection, workdir: &Path, task: &str, slug: Option<&str>, timeout: Duration) -> Result<AgentRun> {
    run_policy(registry, c, workdir, task, slug, timeout, &Policy::for_worktree(workdir), "agente")
}

#[allow(clippy::too_many_arguments)]
fn run_policy(
    registry: &Registry,
    c: &Connection,
    workdir: &Path,
    task: &str,
    slug: Option<&str>,
    timeout: Duration,
    policy: &Policy,
    usage_task: &str,
) -> Result<AgentRun> {
    let mut policy = policy.clone();
    crate::mcp_servers::fill(registry, slug, &c.id, &mut policy)?;
    let result = run_with(c, workdir, task, &policy, timeout);
    let model = c.config_str("model").unwrap_or_else(|| c.provider.clone());
    match &result {
        Ok(r) => crate::ai::record(
            registry,
            c,
            &model,
            usage_task,
            slug,
            (r.input_tokens, r.output_tokens),
            None,
            (!r.ok).then(|| crate::quota::agent_error(&r.output, r.timed_out)).as_deref(),
        )?,
        Err(e) => crate::ai::record(registry, c, &model, usage_task, slug, (None, None), None, Some(&format!("{e:#}")))?,
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn task_goes_whole_into_one_argument() {
        assert_eq!(command_line("codex exec {tarea}", "arregla; rm -rf /"), vec!["codex", "exec", "arregla; rm -rf /"]);
        assert_eq!(command_line("aider --yes", "hola mundo"), vec!["aider", "--yes", "hola mundo"]);
        assert_eq!(command_line("x --msg={tarea}", "a b"), vec!["x", "--msg=a b"]);
    }

    #[test]
    fn reads_usage_when_the_agent_reports_it() {
        let out = "texto\n{\"type\":\"turn\",\"usage\":{\"input_tokens\":10,\"output_tokens\":3}}\n{\"usage\":{\"input_tokens\":25,\"output_tokens\":7}}\n";
        assert_eq!(usage_from_output(out), (Some(25), Some(7)));
        assert_eq!(usage_from_output("sin json"), (None, None));
    }

    /// Un agente falso intenta escribir fuera de su copia y leer una carpeta prohibida.
    #[test]
    fn the_sandbox_keeps_the_agent_inside_its_copy() {
        let root = std::env::temp_dir().join(format!("atalaya-encierro-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let work = root.join("copia");
        let outside = root.join("fuera");
        let secrets = root.join("secretos");
        for d in [&work, &outside, &secrets] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(secrets.join("llave"), "no-deberias-verme").unwrap();
        let script = root.join("agente.sh");
        std::fs::write(
            &script,
            format!(
                "echo \"$1\" > dentro.txt\necho intento > {fuera}/escrito.txt 2>/dev/null && echo ESCRIBIO_FUERA\ncat {secreto} 2>/dev/null && echo LEYO_SECRETO\necho '{{\"usage\":{{\"input_tokens\":5,\"output_tokens\":2}}}}'\n",
                fuera = outside.display(),
                secreto = secrets.join("llave").display()
            ),
        )
        .unwrap();
        let c = Connection {
            id: "falso".into(),
            kind: "agente".into(),
            provider: "agente-personalizado".into(),
            name: "Agente falso".into(),
            config: json!({ "command": format!("/bin/sh {} {{tarea}}", script.display()) }),
            secret_account: None,
            created_at: String::new(),
        };
        let policy = Policy { writable: vec![work.clone()], denied: vec![secrets.clone()], protected: vec![], readable: vec![], env: vec![], mcp: vec![] };
        let run = run_with(&c, &work, "la tarea", &policy, Duration::from_secs(30)).unwrap();
        assert!(run.ok, "{}", run.output);
        assert_eq!(std::fs::read_to_string(work.join("dentro.txt")).unwrap().trim(), "la tarea");
        assert!(!outside.join("escrito.txt").exists(), "escribió fuera de su copia");
        assert!(!run.output.contains("ESCRIBIO_FUERA"));
        assert!(!run.output.contains("no-deberias-verme") && !run.output.contains("LEYO_SECRETO"), "leyó lo prohibido");
        assert_eq!((run.input_tokens, run.output_tokens), (Some(5), Some(2)));

        // Los adjuntos del chat viven dentro de lo prohibido: los lee, pero no los cambia ni
        // ve lo que está al lado.
        let shared = secrets.join("adjuntos");
        std::fs::create_dir_all(&shared).unwrap();
        std::fs::write(shared.join("nota.txt"), "contenido-adjunto").unwrap();
        std::fs::write(
            &script,
            format!(
                "cat {nota}\necho cambio > {nota} 2>/dev/null && echo CAMBIO_ADJUNTO\ncat {secreto} 2>/dev/null && echo LEYO_SECRETO\n",
                nota = shared.join("nota.txt").display(),
                secreto = secrets.join("llave").display()
            ),
        )
        .unwrap();
        let with_files = Policy { readable: vec![shared.clone()], ..policy.clone() };
        let read = run_with(&c, &work, "x", &with_files, Duration::from_secs(30)).unwrap();
        assert!(read.output.contains("contenido-adjunto"), "no leyó el adjunto: {}", read.output);
        assert!(!read.output.contains("CAMBIO_ADJUNTO") && !read.output.contains("LEYO_SECRETO"), "{}", read.output);
        assert_eq!(std::fs::read_to_string(shared.join("nota.txt")).unwrap(), "contenido-adjunto");

        // Puede abrir una terminal virtual (PTY), como hacen los agentes para ejecutar órdenes.
        std::fs::write(&script, "/usr/bin/script -q /dev/null /bin/echo PTY_OK\n").unwrap();
        let pty = run_with(&c, &work, "x", &policy, Duration::from_secs(30)).unwrap();
        assert!(pty.output.contains("PTY_OK"), "no pudo abrir una terminal: {}", pty.output);

        // El tiempo límite se respeta.
        std::fs::write(&script, "sleep 30\n").unwrap();
        let slow = run_with(&c, &work, "x", &policy, Duration::from_millis(500)).unwrap();
        assert!(slow.timed_out && !slow.ok);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// La llave de la conexión llega al agente como variable; las del entorno de Atalaya no.
    #[test]
    fn the_agent_receives_only_its_own_key() {
        let root = std::env::temp_dir().join(format!("atalaya-llave-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let script = root.join("agente.sh");
        std::fs::write(&script, "echo \"llave=$GEMINI_API_KEY gh=$GH_TOKEN\"\n").unwrap();
        let mut policy = Policy { writable: vec![root.clone()], denied: vec![], protected: vec![], readable: vec![], env: vec![], mcp: vec![] };
        policy.env.push(("GEMINI_API_KEY".into(), "llave-de-prueba".into()));
        let run =
            run_template(&format!("/bin/sh {} {{tarea}}", script.display()), &root, "x", &policy, Duration::from_secs(20)).unwrap();
        assert!(run.stdout.contains("llave=llave-de-prueba gh="), "{}", run.stdout);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Qwen Code con un modelo propio: modo --bare, la dirección y el modelo por variables,
    /// y una llave de relleno para un servidor local. Sin modelo propio, todo queda igual.
    #[test]
    fn an_agent_can_use_its_own_model() {
        let mut c = Connection {
            id: "qwen".into(),
            kind: "agente".into(),
            provider: "qwen-code".into(),
            name: "Qwen Code".into(),
            config: serde_json::json!({}),
            secret_account: None,
            created_at: String::new(),
        };
        assert_eq!(template(&c).unwrap(), "qwen --yolo -p {tarea}");
        assert!(key_env(&c).is_empty());
        c.config = serde_json::json!({ "model_base_url": "http://127.0.0.1:11434/v1", "model": "qwen3.5:4b" });
        assert_eq!(template(&c).unwrap(), "qwen --bare --auth-type openai --yolo -p {tarea}");
        assert_eq!(ask_template(&c).unwrap(), "qwen --bare --auth-type openai --yolo -p {tarea}");
        let env = key_env(&c);
        for (k, v) in [("OPENAI_BASE_URL", "http://127.0.0.1:11434/v1"), ("OPENAI_MODEL", "qwen3.5:4b"), ("OPENAI_API_KEY", "local")] {
            assert!(env.contains(&(k.to_string(), v.to_string())), "{env:?}");
        }
        // «localhost» pasa como 127.0.0.1: Node prueba ::1 primero y Ollama no escucha ahí.
        c.config["model_base_url"] = serde_json::json!("http://localhost:11434/v1");
        assert!(key_env(&c).contains(&("OPENAI_BASE_URL".to_string(), "http://127.0.0.1:11434/v1".to_string())));
        // Otro agente no lo toma aunque tenga los mismos campos.
        c.provider = "amp".into();
        c.config["command"] = serde_json::json!("amp -x {tarea}");
        assert_eq!(template(&c).unwrap(), "amp -x {tarea}");
        assert!(key_env(&c).is_empty());
    }

    /// Una carpeta protegida no se escribe aunque esté dentro de lo escribible (un proyecto en /tmp).
    #[test]
    fn protected_folders_stay_read_only() {
        let root = std::env::temp_dir().join(format!("atalaya-protegido-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let project = root.join("proyecto");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join("a.txt"), "original").unwrap();
        let script = root.join("agente.sh");
        std::fs::write(
            &script,
            format!(
                "cat {p}/a.txt\necho cambio > {p}/a.txt 2>/dev/null && echo ESCRIBIO_PROYECTO\necho ok > {r}/libre.txt && echo ESCRIBIO_LIBRE\n",
                p = project.display(),
                r = root.display()
            ),
        )
        .unwrap();
        let policy = Policy { writable: vec![root.clone()], denied: vec![], protected: vec![project.clone()], readable: vec![], env: vec![], mcp: vec![] };
        let run = run_template(&format!("/bin/sh {}", script.display()), &root, "x", &policy, Duration::from_secs(30)).unwrap();
        assert!(run.stdout.contains("original"), "debe poder leer el proyecto: {}", run.output);
        assert!(!run.stdout.contains("ESCRIBIO_PROYECTO"), "escribió en un proyecto protegido");
        assert_eq!(std::fs::read_to_string(project.join("a.txt")).unwrap(), "original");
        assert!(run.stdout.contains("ESCRIBIO_LIBRE"), "{}", run.output);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn in_a_worktree_the_agent_commits_but_cannot_touch_hooks_or_config() {
        let root = std::env::temp_dir().join(format!("atalaya-worktree-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let repo = root.join("repo");
        let work = root.join("copia");
        std::fs::create_dir_all(&repo).unwrap();
        let git = |dir: &Path, args: &[&str]| {
            let out = Command::new("git").arg("-C").arg(dir).args(["-c", "user.name=a", "-c", "user.email=a@b"]).args(args).output().unwrap();
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        git(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("a.txt"), "hola").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-q", "-m", "inicio"]);
        git(&repo, &["worktree", "add", "-q", "-b", "atalaya/mejora-1", &work.to_string_lossy()]);

        let common = repo.join(".git");
        let script = root.join("agente.sh");
        std::fs::write(
            &script,
            format!(
                "echo nuevo > nuevo.txt\ngit add nuevo.txt && git -c user.name=a -c user.email=a@b commit -q -m cambio && echo COMMIT_OK\n\
                 echo malo > {hooks}/post-checkout 2>/dev/null && echo HOOK_ESCRITO\n\
                 git config core.hooksPath /tmp 2>/dev/null && echo CONFIG_ESCRITA\n",
                hooks = common.join("hooks").display()
            ),
        )
        .unwrap();
        let c = Connection {
            id: "falso".into(),
            kind: "agente".into(),
            provider: "agente-personalizado".into(),
            name: "Agente falso".into(),
            config: json!({ "command": format!("/bin/sh {}", script.display()) }),
            secret_account: None,
            created_at: String::new(),
        };
        // Solo su copia, más lo que abre la regla de worktree: así se prueba esa regla y nada más.
        let mut policy = Policy { writable: vec![work.clone()], denied: vec![], protected: vec![], readable: vec![], env: vec![], mcp: vec![] };
        policy.allow_worktree_commits(&work);
        let run = run_with(&c, &work, "x", &policy, Duration::from_secs(30)).unwrap();
        assert!(run.output.contains("COMMIT_OK"), "no pudo confirmar: {}", run.output);
        assert!(!run.output.contains("HOOK_ESCRITO") && !common.join("hooks/post-checkout").exists(), "escribió un hook");
        assert!(!run.output.contains("CONFIG_ESCRITA"), "cambió la configuración del repo");
        assert_eq!(git(&repo, &["log", "-1", "--format=%s", "atalaya/mejora-1"]), "cambio");
        let _ = std::fs::remove_dir_all(&root);
    }
}
