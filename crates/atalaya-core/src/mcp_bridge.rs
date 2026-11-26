//! El MCP de Atalaya para los agentes de terminal (Codex, Gemini CLI, uno propio…).
//!
//! El agente corre encerrado y no puede leer los datos de Atalaya, así que el servidor MCP no
//! puede correr dentro con él. En cada ejecución:
//! 1. Atalaya abre un puerto en 127.0.0.1 solo para esa ejecución, con una contraseña de un
//!    solo uso guardada en un archivo que solo lee esta persona (nunca va en la línea de comando);
//! 2. al agente se le configura como MCP `atalaya mcp-puente <archivo>`, que solo pasa bytes
//!    entre su entrada y salida y ese puerto;
//! 3. por cada conexión con la contraseña correcta, Atalaya lanza fuera del encierro
//!    `atalaya mcp --read-only`: solo las herramientas que leen.
//!
//! Al terminar el agente se cierra el puerto, se detienen esos servidores y se borra la
//! carpeta temporal de esa ejecución.
//!
//! Cómo le llega a cada agente, sin tocar su configuración de siempre:
//! - Codex: `-c mcp_servers.atalaya.command=…` y `-c mcp_servers.atalaya.args=[…]`;
//! - Gemini CLI: `.gemini/settings.json` en la carpeta donde trabaja, solo si es una carpeta
//!   temporal de Atalaya fuera de todo repositorio (las consultas); se quita al terminar. Sus
//!   ajustes de sistema no sirven: exige que su carpeta sea de root;
//! - cualquier otro: la variable `ATALAYA_MCP_CONFIG` y `{mcp_config}` en la plantilla, con la
//!   ruta de un JSON `{"mcpServers": {"atalaya": {"command", "args"}}}`.

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// El marcador de la plantilla que se cambia por la ruta del JSON del MCP.
pub const PLACEHOLDER: &str = "{mcp_config}";

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// El puerto de una ejecución. Al soltarlo se cierra todo.
pub struct Bridge {
    dir: PathBuf,
    cli: PathBuf,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    servers: Arc<Mutex<Vec<Child>>>,
    /// Los ajustes de Gemini que se dejaron: el archivo, lo escrito y lo que había antes.
    gemini: Option<(PathBuf, String, Option<String>)>,
}

impl Bridge {
    /// Con la CLI de Atalaya instalada (la misma que usa Claude Code para el MCP).
    pub fn start() -> Result<Bridge> {
        let cli = PathBuf::from(crate::round::mcp_command(&std::env::current_exe()?));
        Self::start_with(&cli)
    }

    pub fn start_with(cli: &Path) -> Result<Bridge> {
        if !cli.is_file() {
            bail!("no se encontró la CLI de Atalaya («atalaya») para darle su MCP al agente");
        }
        let listener = TcpListener::bind("127.0.0.1:0").context("no se pudo abrir el puerto del MCP")?;
        listener.set_nonblocking(true)?;
        let addr = listener.local_addr()?;
        let token = random_token()?;

        let dir = std::env::temp_dir()
            .join("atalaya-mcp")
            .join(format!("{}-{}", std::process::id(), COUNTER.fetch_add(1, Ordering::Relaxed)));
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode_private()
            .create(&dir)
            .with_context(|| format!("no se pudo crear {}", dir.display()))?;
        let mut bridge = Bridge { dir, cli: cli.to_path_buf(), stop: Arc::new(AtomicBool::new(false)), thread: None, servers: Arc::default(), gemini: None };
        write_private(&bridge.bridge_file(), &json!({ "addr": addr.to_string(), "token": token }).to_string())?;
        write_private(&bridge.config(), &json!({ "mcpServers": { "atalaya": bridge.server() } }).to_string())?;

        let (stop, servers, cli) = (bridge.stop.clone(), bridge.servers.clone(), bridge.cli.clone());
        let thread = std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let (token, cli, servers) = (token.clone(), cli.clone(), servers.clone());
                        std::thread::spawn(move || {
                            let _ = serve_one(stream, &token, &cli, &servers);
                        });
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(50)),
                }
            }
        });
        bridge.thread = Some(thread);
        Ok(bridge)
    }

    /// La carpeta de esta ejecución: el agente la lee, pero no puede escribir en ella.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// El JSON con el servidor, al estilo de Claude Code (`mcpServers`).
    pub fn config(&self) -> PathBuf {
        self.dir.join("mcp.json")
    }

    fn bridge_file(&self) -> PathBuf {
        self.dir.join("puente.json")
    }

    fn server(&self) -> Value {
        json!({ "command": self.cli.display().to_string(), "args": ["mcp-puente", self.bridge_file().display().to_string()] })
    }

    /// Le agrega el MCP a la línea de comando y al entorno del agente, según cuál sea. `cwd`
    /// es la carpeta donde va a trabajar.
    /// `servers`: los MCP externos que también recibe (sus llaves ya vienen en `env`).
    pub fn apply(&mut self, args: &mut Vec<String>, env: &mut Vec<(String, String)>, cwd: &Path, servers: &[crate::mcp_servers::Server]) {
        let config = self.config().display().to_string();
        if !servers.is_empty() {
            // Un agente propio quizás no expande variables: aquí la llave va escrita, en un
            // archivo que solo lee esta persona y que el agente no puede cambiar.
            let mut all = serde_json::Map::new();
            all.insert("atalaya".into(), self.server());
            for s in servers {
                all.insert(s.name.clone(), crate::mcp_servers::entry(s, env, true));
            }
            let _ = std::fs::write(self.config(), json!({ "mcpServers": all }).to_string());
        }
        for arg in args.iter_mut() {
            if arg.contains(PLACEHOLDER) {
                *arg = arg.replace(PLACEHOLDER, &config);
            }
        }
        env.push(("ATALAYA_MCP_CONFIG".into(), config));
        let program = args.first().and_then(|a| Path::new(a).file_name()).map(|n| n.to_string_lossy().into_owned());
        match program.as_deref() {
            Some("codex") => {
                let server = self.server();
                let extra = [
                    "-c".to_string(),
                    format!("mcp_servers.atalaya.command={}", server["command"]),
                    "-c".to_string(),
                    // Un arreglo JSON de textos también es TOML válido.
                    format!("mcp_servers.atalaya.args={}", server["args"]),
                ];
                let mut extra = extra.to_vec();
                let mut more_env = Vec::new();
                for s in servers {
                    let (a, e) = crate::mcp_servers::codex_args(s, env);
                    extra.extend(a);
                    more_env.extend(e);
                }
                args.splice(1..1, extra);
                env.extend(more_env);
            }
            Some("gemini") => {
                if let Some(file) = scratch_settings(cwd) {
                    let previous = std::fs::read_to_string(&file).ok();
                    let text = gemini_settings(previous.as_deref(), self.server(), servers, env).to_string();
                    let written = file.parent().is_some_and(|d| std::fs::create_dir_all(d).is_ok()) && std::fs::write(&file, &text).is_ok();
                    if written {
                        self.gemini = Some((file, text, previous));
                    }
                }
            }
            _ => {}
        }
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        for mut child in self.servers.lock().map(|mut s| std::mem::take(&mut *s)).unwrap_or_default() {
            let _ = child.kill();
            let _ = child.wait();
        }
        // Solo la carpeta temporal de esta ejecución, que creó este mismo puente.
        let _ = std::fs::remove_dir_all(&self.dir);
        // Los ajustes de Gemini quedan como estaban, si nadie los cambió mientras tanto.
        if let Some((file, text, previous)) = self.gemini.take()
            && std::fs::read_to_string(&file).is_ok_and(|now| now == text)
        {
            match previous {
                Some(before) => {
                    let _ = std::fs::write(&file, before);
                }
                None => {
                    let _ = std::fs::remove_file(&file);
                    // Solo si quedó vacía.
                    let _ = file.parent().map(std::fs::remove_dir);
                }
            }
        }
    }
}

/// Dónde dejarle el servidor a Gemini CLI: solo en una carpeta temporal fuera de todo
/// repositorio (así nunca llega a un proyecto ni a un commit).
fn scratch_settings(cwd: &Path) -> Option<PathBuf> {
    let real = cwd.canonicalize().ok()?;
    let in_temp = [std::env::temp_dir(), PathBuf::from("/tmp")]
        .iter()
        .filter_map(|t| t.canonicalize().ok())
        .any(|t| real.starts_with(&t) && real != t);
    let in_repo = real.ancestors().any(|a| a.join(".git").exists());
    (in_temp && !in_repo).then(|| real.join(".gemini").join("settings.json"))
}

/// Los ajustes de Gemini CLI con el servidor de Atalaya, sobre los que ya hubiera.
fn gemini_settings(previous: Option<&str>, server: Value, servers: &[crate::mcp_servers::Server], env: &[(String, String)]) -> Value {
    let mut settings = previous.and_then(|t| serde_json::from_str::<Value>(t).ok()).filter(Value::is_object).unwrap_or_else(|| json!({}));
    let mut server = server;
    // Sin pedir confirmación por cada consulta: todas sus herramientas solo leen.
    server["trust"] = json!(true);
    if !settings["mcpServers"].is_object() {
        settings["mcpServers"] = json!({});
    }
    settings["mcpServers"]["atalaya"] = server;
    for s in servers {
        settings["mcpServers"][s.name.as_str()] = crate::mcp_servers::gemini_entry(s, env);
    }
    settings
}

/// Atiende una conexión: comprueba la contraseña y conecta un servidor de solo lectura.
fn serve_one(mut stream: TcpStream, token: &str, cli: &Path, servers: &Mutex<Vec<Child>>) -> Result<()> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    // Byte a byte, para no leer de más lo que ya es del servidor.
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        if stream.read(&mut byte)? == 0 || line.len() > 256 {
            bail!("conexión sin contraseña");
        }
        if byte[0] == b'\n' {
            break;
        }
        line.push(byte[0]);
    }
    if line != token.as_bytes() {
        bail!("contraseña incorrecta");
    }
    stream.set_read_timeout(None)?;
    let mut child = Command::new(cli)
        .args(["mcp", "--read-only"])
        .env_remove("ATALAYA_CHAT_CONVERSATION")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("no se pudo lanzar el MCP de Atalaya")?;
    let mut stdin = child.stdin.take().context("sin entrada")?;
    let mut stdout = child.stdout.take().context("sin salida")?;
    servers.lock().map_err(|_| anyhow::anyhow!("lista de servidores rota"))?.push(child);
    let mut from_agent = stream.try_clone()?;
    std::thread::spawn(move || {
        // Al cerrarse la entrada, el servidor termina.
        let _ = std::io::copy(&mut from_agent, &mut stdin);
    });
    let _ = std::io::copy(&mut stdout, &mut stream);
    let _ = stream.shutdown(Shutdown::Both);
    Ok(())
}

/// El lado del agente (`atalaya mcp-puente <archivo>`): pasa su entrada al puerto y lo que
/// llega del puerto a su salida. No abre nada de Atalaya: corre dentro del encierro.
pub fn relay(file: &Path) -> Result<()> {
    let info: Value = serde_json::from_str(&std::fs::read_to_string(file).with_context(|| format!("no se pudo leer {}", file.display()))?)?;
    let addr = info["addr"].as_str().context("falta la dirección")?;
    let token = info["token"].as_str().context("falta la contraseña")?;
    let mut stream = TcpStream::connect(addr).context("Atalaya no está escuchando")?;
    stream.write_all(format!("{token}\n").as_bytes())?;
    let mut up = stream.try_clone()?;
    std::thread::spawn(move || {
        let _ = std::io::copy(&mut std::io::stdin().lock(), &mut up);
        let _ = up.shutdown(Shutdown::Write);
    });
    let mut out = std::io::stdout().lock();
    let mut buf = [0u8; 16 * 1024];
    loop {
        let n = stream.read(&mut buf)?;
        if n == 0 {
            return Ok(());
        }
        out.write_all(&buf[..n])?;
        out.flush()?;
    }
}

fn random_token() -> Result<String> {
    let mut bytes = [0u8; 24];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn write_private(path: &Path, text: &str) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path)?;
    file.write_all(text.as_bytes())?;
    Ok(())
}

trait PrivateDir {
    fn mode_private(&mut self) -> &mut Self;
}

impl PrivateDir for std::fs::DirBuilder {
    fn mode_private(&mut self) -> &mut Self {
        use std::os::unix::fs::DirBuilderExt;
        self.mode(0o700)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_cli() -> PathBuf {
        // Cualquier archivo sirve para armar la configuración; estas pruebas no conectan nada.
        PathBuf::from("/bin/cat")
    }

    #[test]
    fn each_agent_gets_the_server_its_own_way() {
        let mut bridge = Bridge::start_with(&fake_cli()).unwrap();
        let config = bridge.config().display().to_string();
        let scratch = std::env::temp_dir().join(format!("atalaya-mcp-prueba-{}", std::process::id()));
        let repo = scratch.join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();

        let mut args: Vec<String> = ["/opt/homebrew/bin/codex", "exec", "--skip-git-repo-check", "la tarea"].map(String::from).into();
        let mut env = vec![];
        bridge.apply(&mut args, &mut env, &repo, &[]);
        assert_eq!(&args[..2], ["/opt/homebrew/bin/codex", "-c"]);
        assert_eq!(args[2], "mcp_servers.atalaya.command=\"/bin/cat\"");
        assert!(args[4].starts_with("mcp_servers.atalaya.args=[\"mcp-puente\",\""), "{}", args[4]);
        assert_eq!(&args[5..], ["exec", "--skip-git-repo-check", "la tarea"]);
        assert_eq!(env, vec![("ATALAYA_MCP_CONFIG".to_string(), config.clone())]);

        // Gemini: nunca dentro de un repositorio; en una carpeta temporal, sí.
        let mut args: Vec<String> = ["gemini", "-p", "la tarea"].map(String::from).into();
        bridge.apply(&mut args, &mut vec![], &repo, &[]);
        assert_eq!(args, ["gemini", "-p", "la tarea"]);
        assert!(!repo.join(".gemini").exists());
        bridge.apply(&mut args, &mut vec![], &scratch, &[]);
        let file = scratch.canonicalize().unwrap().join(".gemini/settings.json");
        let settings: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(settings["mcpServers"]["atalaya"]["trust"], json!(true));
        assert_eq!(settings["mcpServers"]["atalaya"]["args"][0], "mcp-puente");

        let mut args: Vec<String> = ["mi-agente", "--mcp", "{mcp_config}", "x"].map(String::from).into();
        bridge.apply(&mut args, &mut vec![], &repo, &[]);
        assert_eq!(args[2], config);
        let written: Value = serde_json::from_str(&std::fs::read_to_string(&config).unwrap()).unwrap();
        assert_eq!(written["mcpServers"]["atalaya"]["command"], "/bin/cat");

        // La contraseña no queda en la línea de comando ni la puede leer otro usuario.
        let secret = std::fs::read_to_string(bridge.bridge_file()).unwrap();
        let token = serde_json::from_str::<Value>(&secret).unwrap()["token"].as_str().unwrap().to_string();
        assert_eq!(token.len(), 48);
        assert!(!args.iter().any(|a| a.contains(&token)));
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(bridge.bridge_file()).unwrap().permissions().mode() & 0o777, 0o600);

        let dir = bridge.dir.clone();
        drop(bridge);
        assert!(!dir.exists());
        assert!(!file.exists() && !file.parent().unwrap().exists());

        // Si ya había ajustes, quedan como estaban.
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, r#"{"ui":{"theme":"x"}}"#).unwrap();
        let mut bridge = Bridge::start_with(&fake_cli()).unwrap();
        bridge.apply(&mut vec!["gemini".into()], &mut vec![], &scratch, &[]);
        let merged: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(merged["ui"]["theme"], "x");
        assert!(merged["mcpServers"]["atalaya"].is_object());
        drop(bridge);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), r#"{"ui":{"theme":"x"}}"#);
        std::fs::remove_dir_all(&scratch).unwrap();
    }

    #[test]
    fn a_wrong_password_gets_nothing() {
        let bridge = Bridge::start_with(&fake_cli()).unwrap();
        let info: Value = serde_json::from_str(&std::fs::read_to_string(bridge.bridge_file()).unwrap()).unwrap();
        let mut stream = TcpStream::connect(info["addr"].as_str().unwrap()).unwrap();
        stream.write_all(b"otra\n{\"jsonrpc\":\"2.0\"}\n").unwrap();
        let mut got = String::new();
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let _ = stream.read_to_string(&mut got);
        assert!(got.is_empty());
        assert!(bridge.servers.lock().unwrap().is_empty());
    }
}
