//! Servidores MCP externos compartidos (Supabase, GitHub, Context7, Playwright o uno propio).
//! Se agregan una vez en Atalaya y llegan a cada IA cuando trabaja, como las skills:
//! - Claude Code (chat): en su `--mcp-config`, con la llave como `${VARIABLE}` y la variable
//!   solo en el entorno de esa sesión;
//! - agentes de terminal (ver `mcp_bridge`): Codex con `-c mcp_servers…` (la llave por
//!   `env_vars` o `bearer_token_env_var`), Gemini CLI en sus ajustes con `${VARIABLE}`, y los
//!   demás en el JSON de `{mcp_config}`.
//!
//! La llave vive en el Llavero (`mcp:<nombre>`), nunca en la base, en un archivo propio de
//! Atalaya ni en una línea de comando. Se agregan apagados: primero se prueban.

use crate::Registry;
use anyhow::{Context, Result, bail};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::Duration;

pub fn init(registry: &Registry) -> Result<()> {
    registry.conn().execute_batch(
        "CREATE TABLE IF NOT EXISTS mcp_servers (
            name TEXT PRIMARY KEY,
            description TEXT NOT NULL DEFAULT '',
            kind TEXT NOT NULL,
            command TEXT,
            args TEXT NOT NULL DEFAULT '[]',
            url TEXT,
            env TEXT NOT NULL DEFAULT '{}',
            secret_var TEXT,
            enabled INTEGER NOT NULL DEFAULT 0,
            projects TEXT,
            agents TEXT,
            tools TEXT NOT NULL DEFAULT '[]',
            tested_at TEXT,
            installed_at TEXT NOT NULL DEFAULT (datetime('now'))
        );",
    )?;
    Ok(())
}

/// `programa`: se lanza un comando que habla por su entrada y salida. `web`: una dirección
/// HTTP (MCP «streamable HTTP»), con la llave como `Authorization: Bearer`.
pub const PROGRAM: &str = "programa";
pub const WEB: &str = "web";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Server {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub kind: String,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub url: Option<String>,
    /// Variables sin secretos para el programa.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// La variable donde el programa lee su llave (por ejemplo `SUPABASE_ACCESS_TOKEN`).
    #[serde(default)]
    pub secret_var: Option<String>,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub projects: Option<Vec<String>>,
    #[serde(default)]
    pub agents: Option<Vec<String>>,
    /// Las herramientas que mostró en su última prueba.
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub tested_at: Option<String>,
    #[serde(default)]
    pub installed_at: String,
    /// Si tiene llave guardada (nunca su valor).
    #[serde(default)]
    pub has_secret: bool,
}

/// Uno conocido, para agregarlo con un clic.
#[derive(Debug, Clone, Serialize)]
pub struct Preset {
    pub id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub kind: &'static str,
    pub command: Option<&'static str>,
    pub args: &'static [&'static str],
    pub url: Option<&'static str>,
    pub secret_var: Option<&'static str>,
    /// Qué llave pide y dónde se saca. Vacío: no pide.
    pub secret_help: &'static str,
}

pub const PRESETS: &[Preset] = &[
    Preset {
        id: "supabase",
        label: "Supabase",
        description: "Tablas, consultas, registros y migraciones de tus proyectos de Supabase (en solo lectura).",
        kind: PROGRAM,
        command: Some("npx"),
        args: &["-y", "@supabase/mcp-server-supabase@latest", "--read-only"],
        url: None,
        secret_var: Some("SUPABASE_ACCESS_TOKEN"),
        secret_help: "Token personal de Supabase: supabase.com → Account → Access Tokens.",
    },
    Preset {
        id: "github",
        label: "GitHub",
        description: "Repos, issues, pull requests y acciones de GitHub.",
        kind: WEB,
        command: None,
        args: &[],
        url: Some("https://api.githubcopilot.com/mcp/"),
        secret_var: None,
        secret_help: "Token personal de GitHub: github.com → Settings → Developer settings → Personal access tokens.",
    },
    Preset {
        id: "context7",
        label: "Context7",
        description: "Documentación al día de librerías y frameworks.",
        kind: PROGRAM,
        command: Some("npx"),
        args: &["-y", "@upstash/context7-mcp"],
        url: None,
        secret_var: Some("CONTEXT7_API_KEY"),
        secret_help: "Opcional: llave de context7.com para más consultas.",
    },
    Preset {
        id: "playwright",
        label: "Playwright",
        description: "Un navegador sin ventana para abrir páginas, leerlas y sacar capturas.",
        kind: PROGRAM,
        command: Some("npx"),
        args: &["-y", "@playwright/mcp@latest", "--headless"],
        url: None,
        secret_var: None,
        secret_help: "",
    },
];

const COLUMNS: &str = "name, description, kind, command, args, url, env, secret_var, enabled, projects, agents, tools, tested_at, installed_at";

fn account(name: &str) -> String {
    format!("mcp:{name}")
}

fn secret(name: &str) -> Option<String> {
    crate::secrets::get_text(&account(name))
}

/// La variable de Atalaya que lleva la llave de un servidor hasta la IA.
pub fn secret_env(name: &str) -> String {
    format!("ATALAYA_MCP_{}", name.to_uppercase().replace(|c: char| !c.is_ascii_alphanumeric(), "_"))
}

fn row(r: &rusqlite::Row) -> rusqlite::Result<Server> {
    let list = |s: Option<String>| s.and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok());
    let name: String = r.get(0)?;
    Ok(Server {
        has_secret: false,
        description: r.get(1)?,
        kind: r.get(2)?,
        command: r.get(3)?,
        args: list(r.get(4)?).unwrap_or_default(),
        url: r.get(5)?,
        env: serde_json::from_str(&r.get::<_, String>(6)?).unwrap_or_default(),
        secret_var: r.get(7)?,
        enabled: r.get::<_, i64>(8)? != 0,
        projects: list(r.get(9)?),
        agents: list(r.get(10)?),
        tools: list(r.get(11)?).unwrap_or_default(),
        tested_at: r.get(12)?,
        installed_at: r.get(13)?,
        name,
    })
}

fn with_secret_flag(mut s: Server) -> Server {
    s.has_secret = secret(&s.name).is_some();
    s
}

pub fn list(registry: &Registry) -> Result<Vec<Server>> {
    let mut stmt = registry.conn().prepare(&format!("SELECT {COLUMNS} FROM mcp_servers ORDER BY name"))?;
    let rows = stmt.query_map([], row)?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows.into_iter().map(with_secret_flag).collect())
}

pub fn get(registry: &Registry, name: &str) -> Result<Option<Server>> {
    Ok(registry
        .conn()
        .query_row(&format!("SELECT {COLUMNS} FROM mcp_servers WHERE name = ?1"), [name], row)
        .optional()?
        .map(with_secret_flag))
}

fn clean_name(name: &str) -> String {
    let lower = name.trim().to_lowercase();
    let mut out = String::new();
    for c in lower.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_matches('-').chars().take(40).collect()
}

fn valid_var(var: &str) -> bool {
    var.chars().next().is_some_and(|c| c.is_ascii_uppercase() || c == '_')
        && var.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

/// Agrega o cambia un servidor. Uno nuevo queda apagado; al cambiar uno existente se
/// conservan su estado y dónde rige, pero hay que volver a probarlo. `secret`: la llave
/// nueva (sin ella, queda la que había).
pub fn save(registry: &Registry, server: &Server, secret_value: Option<&str>) -> Result<Server> {
    let name = clean_name(&server.name);
    if name.is_empty() {
        bail!("ponle un nombre al servidor");
    }
    if name == "atalaya" {
        bail!("«atalaya» es el nombre del MCP propio de Atalaya: elige otro");
    }
    let (command, url) = match server.kind.as_str() {
        PROGRAM => {
            let command = server.command.as_deref().map(str::trim).filter(|c| !c.is_empty()).context("falta el comando")?;
            (Some(command.to_string()), None)
        }
        WEB => {
            let url = server.url.as_deref().map(str::trim).unwrap_or_default();
            let local = url.starts_with("http://localhost") || url.starts_with("http://127.0.0.1");
            if !url.starts_with("https://") && !local {
                bail!("la dirección debe empezar con https:// (o ser de este Mac)");
            }
            (None, Some(url.to_string()))
        }
        other => bail!("tipo desconocido «{other}»: usa programa o web"),
    };
    let secret_var = server.secret_var.as_deref().map(str::trim).filter(|v| !v.is_empty()).map(String::from);
    if let Some(var) = &secret_var
        && !valid_var(var)
    {
        bail!("«{var}» no sirve como nombre de variable: usa MAYÚSCULAS, números y _");
    }
    for key in server.env.keys() {
        if !valid_var(key) {
            bail!("«{key}» no sirve como nombre de variable: usa MAYÚSCULAS, números y _");
        }
    }
    if let Some(value) = secret_value.map(str::trim).filter(|v| !v.is_empty()) {
        if !crate::secrets::set_text(&account(&name), value)? {
            bail!("no se pudo guardar la llave en el Llavero");
        }
    }
    registry.conn().execute(
        "INSERT INTO mcp_servers (name, description, kind, command, args, url, env, secret_var)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(name) DO UPDATE SET description = ?2, kind = ?3, command = ?4, args = ?5, url = ?6, env = ?7,
           secret_var = ?8, tools = '[]', tested_at = NULL",
        params![
            name,
            server.description.trim(),
            server.kind,
            command,
            serde_json::to_string(&server.args)?,
            url,
            serde_json::to_string(&server.env)?,
            secret_var,
        ],
    )?;
    Ok(get(registry, &name)?.expect("recién guardado"))
}

/// Activa o apaga un servidor, y dónde rige (proyectos e IA; `None` es en todos).
pub fn configure(registry: &Registry, name: &str, enabled: bool, projects: Option<Vec<String>>, agents: Option<Vec<String>>) -> Result<Server> {
    let json = |v: Option<Vec<String>>| v.map(|l| serde_json::to_string(&l).unwrap_or_default());
    let n = registry.conn().execute(
        "UPDATE mcp_servers SET enabled = ?2, projects = ?3, agents = ?4 WHERE name = ?1",
        params![name, enabled as i64, json(projects), json(agents)],
    )?;
    if n == 0 {
        bail!("no hay un servidor «{name}»");
    }
    Ok(get(registry, name)?.expect("existe"))
}

/// Quita el servidor de Atalaya. Su llave queda en el Llavero (se reutiliza si lo vuelves a
/// agregar con el mismo nombre).
pub fn remove(registry: &Registry, name: &str) -> Result<()> {
    if registry.conn().execute("DELETE FROM mcp_servers WHERE name = ?1", [name])? == 0 {
        bail!("no hay un servidor «{name}»");
    }
    Ok(())
}

/* ------------------------------------------------------------------ Entrega */

/// Los servidores activos para este proyecto y esta IA (`claude-code` o el id de la conexión).
pub fn active(registry: &Registry, slug: Option<&str>, ai: &str) -> Result<Vec<Server>> {
    Ok(list(registry)?
        .into_iter()
        .filter(|s| s.enabled)
        .filter(|s| match (&s.projects, slug) {
            (None, _) => true,
            (Some(list), Some(slug)) => list.iter().any(|p| p == slug),
            (Some(_), None) => false,
        })
        .filter(|s| s.agents.as_ref().is_none_or(|list| list.iter().any(|a| a == ai)))
        .collect())
}

/// Lo que recibe una ejecución: los servidores y, aparte, las variables con sus llaves (solo
/// para el entorno de esa IA).
#[derive(Debug, Clone, Default)]
pub struct Delivery {
    pub servers: Vec<Server>,
    pub env: Vec<(String, String)>,
}

pub fn for_run(registry: &Registry, slug: Option<&str>, ai: &str) -> Result<Delivery> {
    let servers = active(registry, slug, ai)?;
    let env = servers.iter().filter_map(|s| secret(&s.name).map(|v| (secret_env(&s.name), v))).collect();
    Ok(Delivery { servers, env })
}

/// Le suma a la política de un agente de terminal sus servidores y sus llaves.
pub fn fill(registry: &Registry, slug: Option<&str>, ai: &str, policy: &mut crate::agents::Policy) -> Result<()> {
    let d = for_run(registry, slug, ai)?;
    policy.mcp.extend(d.servers);
    policy.env.extend(d.env);
    Ok(())
}

fn has_key(s: &Server, env: &[(String, String)]) -> bool {
    env.iter().any(|(k, _)| *k == secret_env(&s.name))
}

fn key_value<'a>(s: &Server, env: &'a [(String, String)]) -> Option<&'a str> {
    env.iter().find(|(k, _)| *k == secret_env(&s.name)).map(|(_, v)| v.as_str())
}

/// La entrada para Claude Code (y el formato `mcpServers` en general). Con `literal`, la
/// llave va escrita (para el JSON de un agente propio, que quizás no expande variables);
/// sin él, como `${VARIABLE}`.
pub fn entry(s: &Server, env: &[(String, String)], literal: bool) -> Value {
    let key = |s: &Server| if literal { key_value(s, env).unwrap_or_default().to_string() } else { format!("${{{}}}", secret_env(&s.name)) };
    match s.kind.as_str() {
        WEB => {
            let mut v = json!({ "type": "http", "url": s.url });
            if has_key(s, env) {
                v["headers"] = json!({ "Authorization": format!("Bearer {}", key(s)) });
            }
            v
        }
        _ => {
            let mut vars: serde_json::Map<String, Value> = s.env.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
            if let Some(var) = &s.secret_var
                && has_key(s, env)
            {
                vars.insert(var.clone(), json!(key(s)));
            }
            json!({ "command": s.command, "args": s.args, "env": vars })
        }
    }
}

/// La entrada para los ajustes de Gemini CLI (expande `${VARIABLE}`).
pub fn gemini_entry(s: &Server, env: &[(String, String)]) -> Value {
    let mut v = entry(s, env, false);
    if s.kind == WEB {
        let headers = v.get("headers").cloned();
        v = json!({ "httpUrl": s.url });
        if let Some(h) = headers {
            v["headers"] = h;
        }
    }
    v
}

/// Las opciones `-c` de Codex para un servidor, y las variables extra que necesita su entorno
/// (Codex le pasa al programa solo las variables que se nombran en `env_vars`).
pub fn codex_args(s: &Server, env: &[(String, String)]) -> (Vec<String>, Vec<(String, String)>) {
    let key = format!("mcp_servers.{}", s.name);
    let mut args = Vec::new();
    let mut extra = Vec::new();
    let mut set = |field: &str, value: String| {
        args.push("-c".to_string());
        args.push(format!("{key}.{field}={value}"));
    };
    // `npx` puede tardar en arrancar dentro del encierro; Codex espera 10 s por defecto.
    set("startup_timeout_sec", "60".to_string());
    match s.kind.as_str() {
        WEB => {
            set("url", json!(s.url).to_string());
            if has_key(s, env) {
                set("bearer_token_env_var", json!(secret_env(&s.name)).to_string());
            }
        }
        _ => {
            set("command", json!(s.command).to_string());
            set("args", json!(s.args).to_string());
            if !s.env.is_empty() {
                let pairs: Vec<String> = s.env.iter().map(|(k, v)| format!("{} = {}", json!(k), json!(v))).collect();
                set("env", format!("{{ {} }}", pairs.join(", ")));
            }
            if let (Some(var), Some(value)) = (&s.secret_var, key_value(s, env)) {
                set("env_vars", json!([var]).to_string());
                extra.push((var.clone(), value.to_string()));
            }
        }
    }
    (args, extra)
}

/* ------------------------------------------------------------------ Prueba */

const INIT: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"atalaya","version":"1"}}}"#;
const INITIALIZED: &str = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
const LIST: &str = r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#;

fn tool_names(result: &Value) -> Result<Vec<String>> {
    if let Some(e) = result.get("error") {
        bail!("el servidor respondió error: {}", e["message"].as_str().unwrap_or("sin detalle"));
    }
    Ok(result["result"]["tools"]
        .as_array()
        .context("el servidor no devolvió su lista de herramientas")?
        .iter()
        .filter_map(|t| t["name"].as_str().map(String::from))
        .collect())
}

/// Prueba un servidor: lo arranca (un programa, dentro del mismo encierro que los agentes),
/// le pide sus herramientas y las anota.
pub fn test(registry: &Registry, name: &str) -> Result<Vec<String>> {
    let s = get(registry, name)?.with_context(|| format!("no hay un servidor «{name}»"))?;
    let env: Vec<(String, String)> = secret(name).map(|v| (secret_env(name), v)).into_iter().collect();
    let tools = match s.kind.as_str() {
        WEB => test_web(&s, key_value(&s, &env))?,
        _ => test_program(&s, &env)?,
    };
    registry.conn().execute(
        "UPDATE mcp_servers SET tools = ?2, tested_at = datetime('now') WHERE name = ?1",
        params![name, serde_json::to_string(&tools)?],
    )?;
    Ok(tools)
}

fn test_program(s: &Server, env: &[(String, String)]) -> Result<Vec<String>> {
    let command = s.command.as_deref().context("falta el comando")?;
    let program = crate::agents::find_program(command).with_context(|| format!("no se encontró «{command}» instalado"))?;
    let scratch = std::env::temp_dir().join(format!("atalaya-mcp-prueba-{}", std::process::id()));
    std::fs::create_dir_all(&scratch)?;
    let policy = crate::agents::Policy::for_workdir(&scratch);
    let mut cmd = Command::new("/usr/bin/sandbox-exec");
    cmd.arg("-p").arg(crate::agents::profile(&policy)).arg(&program).args(&s.args);
    cmd.current_dir(&scratch).env_clear().stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    for k in ["HOME", "USER", "LANG", "TMPDIR"] {
        if let Ok(v) = std::env::var(k) {
            cmd.env(k, v);
        }
    }
    cmd.env("PATH", crate::worker::developer_path());
    cmd.envs(&s.env);
    if let (Some(var), Some(value)) = (&s.secret_var, key_value(s, env)) {
        cmd.env(var, value);
    }
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    let mut child = cmd.spawn().context("no se pudo lanzar el servidor")?;
    let group = child.id();
    let mut stdin = child.stdin.take().context("sin entrada")?;
    let stdout = child.stdout.take().context("sin salida")?;
    let stderr = child.stderr.take().context("sin salida de errores")?;
    let errors = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = std::io::Read::read_to_string(&mut BufReader::new(stderr), &mut s);
        s
    });
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if let Ok(v) = serde_json::from_str::<Value>(&line)
                && v["id"] == json!(2)
            {
                let _ = tx.send(v);
                break;
            }
        }
    });
    writeln!(stdin, "{INIT}\n{INITIALIZED}\n{LIST}")?;
    // La primera vez, `npx` descarga el paquete: puede tardar.
    let answer = rx.recv_timeout(Duration::from_secs(120));
    drop(stdin);
    let _ = Command::new("/bin/kill").args(["-9", &format!("-{group}")]).status();
    let _ = child.kill();
    let _ = child.wait();
    match answer {
        Ok(v) => tool_names(&v),
        Err(_) => {
            let tail: String = errors.join().unwrap_or_default().trim().chars().rev().take(600).collect::<Vec<_>>().into_iter().rev().collect();
            bail!("el servidor no respondió{}", if tail.is_empty() { String::new() } else { format!(": {tail}") })
        }
    }
}

/// Una respuesta HTTP de MCP: JSON, o eventos (`data: {...}`).
fn parse_reply(text: &str, id: u64) -> Option<Value> {
    let direct = serde_json::from_str::<Value>(text).ok().into_iter();
    let events = text.lines().filter_map(|l| l.strip_prefix("data:")).filter_map(|d| serde_json::from_str::<Value>(d.trim()).ok());
    direct.chain(events).find(|v| v["id"] == json!(id))
}

fn test_web(s: &Server, key: Option<&str>) -> Result<Vec<String>> {
    let url = s.url.as_deref().context("falta la dirección")?;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        .http_status_as_error(false)
        .max_redirects(0)
        .build()
        .into();
    let auth = key.map(|k| format!("Bearer {k}"));
    let post = |body: &str, session: Option<&str>| -> Result<(u16, Option<String>, String)> {
        let mut req = agent.post(url).header("Content-Type", "application/json").header("Accept", "application/json, text/event-stream");
        if let Some(a) = &auth {
            req = req.header("Authorization", a);
        }
        if let Some(sid) = session {
            req = req.header("Mcp-Session-Id", sid);
        }
        let mut resp = req.send(body.to_string()).with_context(|| format!("no se pudo llegar a {url}"))?;
        let sid = resp.headers().get("mcp-session-id").and_then(|v| v.to_str().ok()).map(String::from);
        Ok((resp.status().as_u16(), sid, resp.body_mut().read_to_string().unwrap_or_default()))
    };
    let (code, session, text) = post(INIT, None)?;
    match code {
        200..=299 => {}
        401 | 403 => bail!("el servidor no aceptó la llave (error {code})"),
        _ => bail!("el servidor respondió error {code}"),
    }
    parse_reply(&text, 1).context("el servidor no respondió como MCP")?;
    let _ = post(INITIALIZED, session.as_deref());
    let (_, _, text) = post(LIST, session.as_deref())?;
    tool_names(&parse_reply(&text, 2).context("el servidor no devolvió su lista de herramientas")?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn program(name: &str) -> Server {
        Server {
            name: name.into(),
            description: String::new(),
            kind: PROGRAM.into(),
            command: Some("npx".into()),
            args: vec!["-y".into(), "paquete".into()],
            url: None,
            env: BTreeMap::from([("MODO".to_string(), "lectura".to_string())]),
            secret_var: Some("SERVICIO_TOKEN".into()),
            enabled: false,
            projects: None,
            agents: None,
            tools: vec![],
            tested_at: None,
            installed_at: String::new(),
            has_secret: false,
        }
    }

    #[test]
    fn each_ai_gets_the_server_without_the_key_in_sight() {
        let s = program("Mi Servicio");
        let env = vec![(secret_env("mi-servicio"), "llave-123".to_string())];
        let s = Server { name: "mi-servicio".into(), ..s };

        let claude = entry(&s, &env, false);
        assert_eq!(claude["env"]["SERVICIO_TOKEN"], "${ATALAYA_MCP_MI_SERVICIO}");
        assert_eq!(claude["env"]["MODO"], "lectura");
        assert!(!claude.to_string().contains("llave-123"));
        assert_eq!(entry(&s, &env, true)["env"]["SERVICIO_TOKEN"], "llave-123");

        let (args, extra) = codex_args(&s, &env);
        assert!(!args.join(" ").contains("llave-123"), "{args:?}");
        assert!(args.contains(&"mcp_servers.mi-servicio.command=\"npx\"".to_string()));
        assert!(args.contains(&"mcp_servers.mi-servicio.env_vars=[\"SERVICIO_TOKEN\"]".to_string()));
        assert!(args.contains(&"mcp_servers.mi-servicio.env={ \"MODO\" = \"lectura\" }".to_string()));
        assert_eq!(extra, vec![("SERVICIO_TOKEN".to_string(), "llave-123".to_string())]);

        let web = Server { kind: WEB.into(), command: None, url: Some("https://mcp.ejemplo.cl/mcp".into()), ..s.clone() };
        assert_eq!(entry(&web, &env, false)["headers"]["Authorization"], "Bearer ${ATALAYA_MCP_MI_SERVICIO}");
        let g = gemini_entry(&web, &env);
        assert_eq!(g["httpUrl"], "https://mcp.ejemplo.cl/mcp");
        assert!(g.get("url").is_none());
        let (args, extra) = codex_args(&web, &env);
        assert!(args.contains(&"mcp_servers.mi-servicio.bearer_token_env_var=\"ATALAYA_MCP_MI_SERVICIO\"".to_string()));
        assert!(extra.is_empty());

        // Sin llave guardada no se nombra ninguna variable.
        assert!(entry(&s, &[], false)["env"].get("SERVICIO_TOKEN").is_none());
        assert!(entry(&web, &[], false).get("headers").is_none());
    }

    #[test]
    fn saved_off_scoped_and_checked() {
        let dir = std::env::temp_dir().join(format!("atalaya-mcps-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let r = Registry::open(&dir.join("a.db")).unwrap();
        let mut s = program("Mi Servicio!");
        s.secret_var = None;
        let saved = save(&r, &s, None).unwrap();
        assert_eq!(saved.name, "mi-servicio");
        assert!(!saved.enabled);
        assert!(active(&r, None, "claude-code").unwrap().is_empty());
        configure(&r, "mi-servicio", true, Some(vec!["faro".into()]), Some(vec!["codex-cli".into()])).unwrap();
        assert_eq!(active(&r, Some("faro"), "codex-cli").unwrap().len(), 1);
        assert!(active(&r, Some("faro"), "claude-code").unwrap().is_empty());
        assert!(active(&r, None, "codex-cli").unwrap().is_empty());
        // Cambiarlo conserva su estado.
        let again = save(&r, &Server { description: "otra".into(), ..s.clone() }, None).unwrap();
        assert!(again.enabled && again.projects == Some(vec!["faro".to_string()]));

        assert!(save(&r, &Server { name: "atalaya".into(), ..s.clone() }, None).is_err());
        assert!(save(&r, &Server { kind: WEB.into(), url: Some("http://ejemplo.cl".into()), ..s.clone() }, None).is_err());
        assert!(save(&r, &Server { secret_var: Some("mal nombre".into()), ..s.clone() }, None).is_err());
        remove(&r, "mi-servicio").unwrap();
        assert!(list(&r).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reads_json_and_event_replies() {
        let sse = "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"tools\":[{\"name\":\"a\"},{\"name\":\"b\"}]}}\n\n";
        assert_eq!(tool_names(&parse_reply(sse, 2).unwrap()).unwrap(), vec!["a", "b"]);
        let plain = r#"{"jsonrpc":"2.0","id":2,"error":{"message":"no"}}"#;
        assert!(tool_names(&parse_reply(plain, 2).unwrap()).is_err());
    }
}
