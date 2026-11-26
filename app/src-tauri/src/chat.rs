//! La vista Agente: una terminal de IA dentro de Atalaya.
//!
//! Responde el rol «asistente» de Ajustes → IA, o el modelo que se elija en la conversación:
//! - **Claude Code**, con todas sus herramientas: lee y edita código, ejecuta órdenes, usa
//!   git y todas las herramientas de Atalaya. Tres modos:
//!   - `preguntar` (el normal): cada acción que cambia algo espera tu visto bueno en la
//!     conversación (herramienta MCP `aprobar_accion` y la tabla `approvals`);
//!   - `autonomo`: actúa sin preguntar, como `--permission-mode bypassPermissions`;
//!   - `lectura`: solo lee y anota mejoras.
//! - **Un modelo por API**, que conversa con el paquete de contexto en JSON.

use atalaya_core::Registry;
use atalaya_core::ai::{Message, Request};
use atalaya_core::orchestrator::{self, CLAUDE_CODE, Candidate};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter};

/// Lo que se usa sin preguntar en el modo normal: leer y buscar.
const READ_TOOLS: &str = "Read,Grep,Glob,LS,WebSearch,TodoWrite,mcp__atalaya__list_projects,\
mcp__atalaya__get_project_context,mcp__atalaya__recent_commits,mcp__atalaya__project_status,mcp__atalaya__list_flags,\
mcp__atalaya__list_improvements,mcp__atalaya__claude_usage,mcp__atalaya__ai_usage,mcp__atalaya__ai_roles,\
mcp__atalaya__list_decisions,mcp__atalaya__github_issues,mcp__atalaya__admin_controls,mcp__atalaya__radar_events,\
mcp__atalaya__set_improvement_status,mcp__atalaya__list_memories,mcp__atalaya__proponer_memoria,Skill";

/// Mover una mejora de etapa: es solo el tablero de Atalaya, no pide permiso.
const STAGE: &str = "mcp__atalaya__set_improvement_status";

/// El modo de solo lectura, como era antes: leer y anotar mejoras.
const READ_ONLY: &str = "Read,Grep,Glob,Skill,mcp__atalaya__list_projects,mcp__atalaya__get_project_context,\
mcp__atalaya__recent_commits,mcp__atalaya__project_status,mcp__atalaya__list_flags,\
mcp__atalaya__list_improvements,mcp__atalaya__add_improvement,mcp__atalaya__list_memories,mcp__atalaya__proponer_memoria";

/// Las herramientas para coordinar a las demás IA (ver `atalaya_core::delegate`).
const CONSULT: &str = "mcp__atalaya__delegar_consulta";
const WORK: &str = "mcp__atalaya__delegar_trabajo";
const CONSULTS: &str = "mcp__atalaya__delegar_consultas";
const WORKS: &str = "mcp__atalaya__delegar_trabajos";

const PROMPT: &str = "Eres el asistente de Atalaya, la consola con la que el usuario vigila y mantiene sus proyectos. \
Responde en español, directo y en texto simple. Usa las herramientas de Atalaya para consultar el estado real antes de \
afirmar algo.";

/// Solo Claude Code tiene la herramienta para proponer memoria.
const MEMORY: &str = " Si en la conversación aparece algo durable sobre un proyecto o sobre cómo trabaja el usuario \
(una convención, una decisión tomada, una preferencia), propónlo con proponer_memoria para que todas las IA lo \
recuerden; el usuario lo aprueba. No propongas lo obvio, lo pasajero ni lo que ya está en la memoria o las instrucciones.";

const POWERS: &str = " Puedes hacer todo lo que haría una persona en la terminal: leer y editar código, ejecutar \
órdenes, usar git, instalar dependencias, correr pruebas y builds, y usar todas las herramientas de Atalaya \
(interruptores, administración de las apps, mejoras y decisiones). Cuando el usuario te pida resolver algo, resuélvelo \
completo: investiga, cambia lo necesario, verifica con pruebas o build y termina con un resumen de qué cambiaste. \
Si necesitas una decisión del usuario, pregúntala en tu respuesta y termina ahí. \
Lleva las mejoras por sus etapas en el tablero: antes de trabajar en un proyecto mira list_improvements, y si lo que \
vas a hacer ya es una mejora anotada (o la anotaste tú), muévela con set_improvement_status: 'en_curso' al empezar, \
'en_revision' cuando el cambio esté listo y espere el visto bueno del usuario (con la rama o el PR), 'hecha' cuando \
quede aplicado y verificado, y 'aceptada' si queda a medias. Agrega siempre una nota corta de qué hiciste o qué falta. \
Nunca dejes como 'propuesta' algo que ya hiciste. \
Las decisiones de la cola (list_decisions) son del usuario: apruébalas o descártalas con resolve_decision solo cuando \
te lo pida. Bajo cada mejora o decisión que tocas, el chat le muestra sus botones, así que no hace falta que le \
expliques cómo hacerlo en otra vista.";

const ASKING: &str = " Cada acción que cambia algo espera el visto bueno del usuario en la app. Si la rechaza, no \
busques otro camino para hacer lo mismo: pregunta qué prefiere.";

const AUTONOMOUS: &str = " Actúas sin pedir permiso. Aun así, antes de algo irreversible o que toque producción \
(borrar datos o archivos que no creaste, desplegar, git push --force, cambiar datos de clientes) detente y pregúntalo \
en tu respuesta.";

const READ_ONLY_PROMPT: &str = " En este modo solo puedes leer: no cambias código ni interruptores. Puedes anotar \
una mejora con add_improvement cuando el usuario lo pida o lo acepte.";

const API_PROMPT: &str = "Eres el asistente de Atalaya, la consola con la que el usuario vigila sus proyectos. \
Responde en español, breve y directo, en texto simple sin tablas. Abajo va el estado real de los proyectos en JSON \
(revisiones, mejoras abiertas, decisiones pendientes y eventos recientes): básate en eso y no inventes datos; si algo \
no está, dilo. Desde este chat no puedes cambiar nada: para eso, el usuario elige Claude Code.";

#[derive(Clone, Serialize)]
struct ChatEvent {
    /// Identifica a qué pregunta pertenece el evento.
    request: String,
    /// `delta` (texto que va llegando), `texto`, `herramienta`, `resultado`, `fin` o `error`.
    kind: &'static str,
    text: String,
    /// El detalle: la orden, el archivo… En `resultado`, `error` si la herramienta falló.
    detail: Option<String>,
    /// Sesión, para continuar la conversación.
    session: Option<String>,
    /// Quién da este paso y bajo qué llamada, para ver quién hace qué (solo con Claude Code).
    #[serde(skip_serializing_if = "Option::is_none")]
    step: Option<Step>,
}

/// Un paso de Claude Code o de uno de sus subagentes.
#[derive(Clone, Default, Serialize)]
struct Step {
    /// El de la herramienta: su resultado llega con el mismo.
    id: Option<String>,
    /// La llamada que lanzó al subagente que da este paso; `None` si es Claude Code.
    parent: Option<String>,
    /// Quién: «Claude Code» o el subagente.
    actor: String,
    /// En una llamada que lanza un subagente, cómo se llama ese subagente.
    #[serde(skip_serializing_if = "Option::is_none")]
    spawns: Option<String>,
}

/// Conversaciones con agentes de terminal: lo conversado, para dárselo en el siguiente turno.
static CLI_HISTORY: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

/// Conversaciones con modelos por API: la historia vive mientras la app está abierta.
static HISTORY: Mutex<Option<HashMap<String, Vec<Message>>>> = Mutex::new(None);

/// Procesos de Claude Code en marcha, por conversación, para poder detenerlos.
static RUNNING: Mutex<Option<HashMap<String, u32>>> = Mutex::new(None);

type Emit<'a> = dyn Fn(&'static str, String, Option<String>, Option<String>) + 'a;

/// Como `Emit`, para los pasos con su autor.
type EmitStep<'a> = dyn Fn(&'static str, String, Option<String>, Step) + 'a;

struct Turn<'a> {
    request: &'a str,
    message: &'a str,
    session: Option<&'a str>,
    slug: Option<&'a str>,
    mode: &'a str,
    conversation: &'a str,
    /// Los archivos que adjuntaste a este mensaje.
    attachments: &'a [atalaya_core::attachments::Attachment],
}

fn run(app: &AppHandle, turn: &Turn, model: Option<&str>) -> anyhow::Result<()> {
    let emit = |kind: &'static str, text: String, detail: Option<String>, session: Option<String>| {
        let _ = app.emit("chat", ChatEvent { request: turn.request.to_string(), kind, text, detail, session, step: None });
    };
    let emit_step = |kind: &'static str, text: String, detail: Option<String>, step: Step| {
        let _ = app.emit(
            "chat",
            ChatEvent { request: turn.request.to_string(), kind, text, detail, session: None, step: Some(step) },
        );
    };
    let registry = Registry::open_default()?;
    // Una conversación sigue con quien la empezó.
    if let Some(session) = turn.session {
        if let Some(rest) = session.strip_prefix("cli:") {
            let id = rest.split(':').next().unwrap_or_default();
            let c = atalaya_core::connections::get(&registry, id)?
                .ok_or_else(|| anyhow::anyhow!("el agente de esta conversación ya no existe; empieza una nueva"))?;
            return run_cli(&registry, &c, &emit, turn);
        }
        if let Some(rest) = session.strip_prefix("api:") {
            let id = rest.split(':').next().unwrap_or_default();
            let c = atalaya_core::connections::get(&registry, id)?
                .ok_or_else(|| anyhow::anyhow!("la conexión de esta conversación ya no existe; empieza una nueva"))?;
            return run_api(&registry, &c, &emit, turn);
        }
        return run_claude(&registry, &emit, &emit_step, turn);
    }
    let start = |who: &Candidate| match who {
        Candidate::ClaudeCode => run_claude(&registry, &emit, &emit_step, turn),
        Candidate::Connection(c) if c.kind == "agente" => run_cli(&registry, c, &emit, turn),
        Candidate::Connection(c) => run_api(&registry, c, &emit, turn),
    };
    match model.filter(|m| !m.is_empty()) {
        Some(CLAUDE_CODE) => start(&Candidate::ClaudeCode),
        Some(id) => {
            let c = atalaya_core::connections::get(&registry, id)?
                .ok_or_else(|| anyhow::anyhow!("no existe la conexión '{id}'"))?;
            start(&Candidate::Connection(c))
        }
        None => orchestrator::attempt(&registry, "asistente", &[], start).map(|_| ()),
    }
}

/// Responde con un modelo por API. La historia se guarda por sesión.
fn run_api(registry: &Registry, c: &atalaya_core::connections::Connection, emit: &Emit, turn: &Turn) -> anyhow::Result<()> {
    let session = turn.session.map(String::from).unwrap_or_else(|| format!("api:{}:{}", c.id, turn.request));
    emit("quien", Candidate::Connection(c.clone()).label(), None, None);
    let context = match turn.slug {
        Some(slug) => atalaya_core::context::pack(registry, slug)?,
        None => atalaya_core::context::portfolio(registry)?,
    }
    .to_string();
    let context: String = if context.chars().count() > 40_000 { context.chars().take(40_000).collect() } else { context };
    let mut history = {
        let mut all = HISTORY.lock().unwrap();
        all.get_or_insert_with(HashMap::new).get(&session).cloned().unwrap_or_default()
    };
    let (extra, images) = atalaya_core::attachments::for_api(turn.attachments);
    let mut message = Message::user(format!("{}{extra}", turn.message));
    message.images = images;
    history.push(message);
    let guidance = atalaya_core::skills::for_ai(registry, turn.slug, &c.id, atalaya_core::skills::Reader::Inline)?;
    let req = Request {
        system: Some(format!("{API_PROMPT}{guidance}\n\n{context}")),
        messages: history.clone(),
        max_tokens: 2048,
        task: "chat".into(),
        slug: turn.slug.map(String::from),
        ..Default::default()
    };
    let reply = atalaya_core::ai::complete(registry, c, &req)?;
    history.push(Message::assistant(reply.text.clone()));
    HISTORY.lock().unwrap().get_or_insert_with(HashMap::new).insert(session.clone(), history);
    emit("texto", reply.text, None, None);
    emit("fin", String::new(), None, Some(session));
    Ok(())
}

/// Responde con un agente de terminal (Codex, Gemini CLI, Kimi…) dentro del encierro de Atalaya.
/// No sabe pedir permiso desde la app: en `autonomo` escribe en las carpetas de los
/// proyectos; en los demás modos solo lee. Su salida llega mientras la escribe.
fn run_cli(registry: &Registry, c: &atalaya_core::connections::Connection, emit: &Emit, turn: &Turn) -> anyhow::Result<()> {
    let session = turn.session.map(String::from).unwrap_or_else(|| format!("cli:{}:{}", c.id, turn.request));
    emit("quien", Candidate::Connection(c.clone()).label(), None, None);
    let writes = turn.mode == "autonomo";
    // Si puede todo, la plantilla de trabajar; si no, la de responder.
    let template = if writes { atalaya_core::agents::template(c)? } else { atalaya_core::agents::ask_template(c)? };
    if turn.mode == "preguntar" {
        emit(
            "resultado",
            format!("{} no puede pedirte permiso desde Atalaya, así que trabaja en solo lectura. Para que haga cambios, elige «Puede todo sin preguntar».", c.name),
            None,
            None,
        );
    }

    let folders = project_folders(registry)?;
    let project = match turn.slug {
        Some(slug) => registry.get(slug)?,
        None => None,
    };
    let cwd = project
        .as_ref()
        .and_then(|p| p.local_path.clone())
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
        .or_else(|| atalaya_core::discover::roots(registry).ok().and_then(|r| r.into_iter().find(|p| p.is_dir())))
        .unwrap_or_else(std::env::temp_dir);

    // El encierro: en solo lectura escribe en una carpeta temporal; si puede todo, también
    // en las carpetas de los proyectos.
    let scratch = std::env::temp_dir().join(format!("atalaya-chat-{}", c.id));
    std::fs::create_dir_all(&scratch)?;
    let mut policy = atalaya_core::agents::Policy::for_workdir(&scratch);
    policy.env = atalaya_core::agents::key_env(c);
    atalaya_core::mcp_servers::fill(registry, turn.slug, &c.id, &mut policy)?;
    if writes {
        policy.writable.extend(folders.iter().map(|(_, p)| p.clone()));
    } else {
        // Aunque un proyecto quede dentro de lo escribible (por ejemplo, en /tmp), no se toca.
        policy.protected = folders.iter().map(|(_, p)| p.clone()).collect();
    }

    let context = match turn.slug {
        Some(slug) => atalaya_core::context::pack(registry, slug)?,
        None => atalaya_core::context::portfolio(registry)?,
    }
    .to_string();
    let mut prompt = PROMPT.to_string();
    prompt.push_str(if writes { POWERS } else { " En este turno solo puedes leer: no cambies archivos." });
    if writes {
        prompt.push_str(AUTONOMOUS);
    }
    if !folders.is_empty() {
        let list: Vec<String> = folders.iter().map(|(name, path)| format!("{name}: {}", path.display())).collect();
        prompt.push_str(&format!(" Carpetas de los proyectos: {}.", list.join("; ")));
    }
    prompt.push_str(&atalaya_core::skills::for_ai(registry, turn.slug, &c.id, atalaya_core::skills::Reader::Files)?);
    prompt.push_str(&format!("\n\nEstado de los proyectos según Atalaya (JSON):\n{}", clip(&context, 20_000)));
    let history = CLI_HISTORY.lock().unwrap().get_or_insert_with(HashMap::new).get(&session).cloned().unwrap_or_default();
    if !history.is_empty() {
        prompt.push_str(&format!("\n\nLo conversado hasta ahora:\n{}", clip_start(&history, 20_000)));
    }
    prompt.push_str(&format!("\n\nMensaje del usuario:\n{}{}", turn.message, atalaya_core::attachments::note(turn.attachments)));

    let mut child = atalaya_core::agents::spawn(&template, &cwd, &prompt, &policy)?;
    RUNNING.lock().unwrap().get_or_insert_with(HashMap::new).insert(turn.conversation.to_string(), child.id());
    let stderr = child.stderr.take().expect("stderr");
    let errors = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = std::io::Read::read_to_string(&mut BufReader::new(stderr), &mut s);
        s
    });
    let mut reply = String::new();
    for line in BufReader::new(child.stdout.take().expect("stdout")).lines() {
        let Ok(line) = line else { break };
        let line = atalaya_core::clis::clean_line(&c.provider, &line);
        let chunk = if reply.is_empty() { line.to_string() } else { format!("\n{line}") };
        reply.push_str(&chunk);
        emit("delta", chunk, None, None);
    }
    let status = child.wait()?;
    let errors = errors.join().unwrap_or_default();
    let stopped = RUNNING.lock().unwrap().get_or_insert_with(HashMap::new).remove(turn.conversation).is_none();
    let model = c.config_str("model").unwrap_or_else(|| c.provider.clone());
    let failure = if stopped {
        Some("Detenido.".to_string())
    } else if !status.success() || reply.trim().is_empty() {
        let tail: String = errors.trim().chars().rev().take(800).collect::<Vec<_>>().into_iter().rev().collect();
        Some(if tail.is_empty() { format!("{} terminó sin responder", c.name) } else { tail })
    } else {
        None
    };
    atalaya_core::ai::record(registry, c, &model, "chat", turn.slug, (None, None), None, failure.as_deref())?;
    let mut all = history;
    all.push_str(&format!("\nUsuario: {}\n{}: {}\n", turn.message, c.name, reply.trim()));
    CLI_HISTORY.lock().unwrap().get_or_insert_with(HashMap::new).insert(session.clone(), all);
    match failure {
        Some(why) => emit("error", why, None, Some(session)),
        None => emit("fin", String::new(), None, Some(session)),
    }
    Ok(())
}

/// Lo último de un texto largo.
fn clip_start(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_string();
    }
    format!("…{}", text.chars().skip(count - max).collect::<String>())
}

fn clip(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max {
        return text.to_string();
    }
    format!("{}…", text.chars().take(max).collect::<String>())
}

/// Lo esencial de una llamada a herramienta, para mostrarlo como en una terminal.
fn tool_detail(name: &str, input: &Value) -> Option<String> {
    let s = |k: &str| input[k].as_str().map(String::from);
    match name {
        "Bash" => s("command"),
        "Read" | "Edit" | "MultiEdit" | "Write" => s("file_path"),
        "NotebookEdit" => s("notebook_path"),
        "Grep" | "Glob" => s("pattern"),
        "WebFetch" => s("url"),
        "WebSearch" => s("query"),
        "Task" | "Agent" => s("description"),
        // Las de Atalaya llegan como plugin: «atalaya:nombre».
        "Skill" => s("skill").or_else(|| s("command")).map(|n| n.trim_start_matches("atalaya:").to_string()),
        // A quién le encarga y qué, para seguir el reparto desde el chat.
        CONSULT | WORK => {
            let who = s("ia").filter(|w| !w.is_empty()).unwrap_or_else(|| "según tus roles".into());
            Some(format!("{who}: {}", clip(&s("tarea").unwrap_or_default(), 200)))
        }
        // Un reparto: a cuántos y a quiénes.
        CONSULTS | WORKS => {
            let orders = input["encargos"].as_array().cloned().unwrap_or_default();
            let who: Vec<String> = orders
                .iter()
                .map(|o| {
                    let ia = o["ia"].as_str().filter(|w| !w.is_empty()).unwrap_or("según tus roles");
                    match o["proyecto"].as_str() {
                        Some(p) if name == WORKS => format!("{ia} en {p}"),
                        _ => ia.to_string(),
                    }
                })
                .collect();
            Some(format!("{} encargos: {}", orders.len(), who.join(" · ")))
        }
        "mcp__atalaya__resolve_decision" => Some(format!("#{} → {}", input["id"], s("accion").unwrap_or_default())),
        STAGE => {
            let stage = s("status").unwrap_or_default().replace('_', " ");
            let note = s("note").map(|n| format!(" · {}", clip(&n, 160))).unwrap_or_default();
            Some(format!("#{} → {stage}{note}", input["id"]))
        }
        // Las de Atalaya y las de los MCP externos: sus datos, cortos.
        _ if name.starts_with("mcp__") => {
            let text = input.to_string();
            (text != "{}").then(|| clip(&text, 200))
        }
        _ => None,
    }
}

/// El nombre que se muestra: las de Atalaya, sin prefijo; las de un MCP externo, «servidor · herramienta».
fn tool_name(name: &str) -> String {
    if let Some(own) = name.strip_prefix("mcp__atalaya__") {
        return own.to_string();
    }
    match name.strip_prefix("mcp__").and_then(|rest| rest.split_once("__")) {
        Some((server, tool)) => format!("{server} · {tool}"),
        None => name.to_string(),
    }
}

/// El texto que devolvió una herramienta (a veces viene en bloques).
fn tool_output(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks.iter().filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

/// Las carpetas de los proyectos registrados que existen en este Mac.
fn project_folders(registry: &Registry) -> anyhow::Result<Vec<(String, PathBuf)>> {
    Ok(registry
        .list()?
        .into_iter()
        .filter_map(|p| {
            let path = PathBuf::from(p.local_path?);
            path.is_dir().then_some((p.name, path))
        })
        .collect())
}

/// Responde con Claude Code y anota el turno en el consumo, aunque se detenga o falle.
fn run_claude(registry: &Registry, emit: &Emit, emit_step: &EmitStep, turn: &Turn) -> anyhow::Result<()> {
    emit("quien", CLAUDE.into(), None, None);
    atalaya_core::ai::tracked_claude(registry, "chat", turn.slug, |last| claude_turn(registry, emit, emit_step, turn, last))
}

const CLAUDE: &str = "Claude Code";

/// Cómo se llama en el chat el subagente que lanza esta llamada a `Agent` (antes `Task`).
fn subagent_label(input: &Value) -> String {
    let kind = input["subagent_type"].as_str().filter(|k| !k.is_empty() && *k != "general-purpose");
    let what = input["description"].as_str().unwrap_or_default();
    let who = match kind {
        Some(kind) => format!("Subagente {kind}"),
        None => "Subagente".to_string(),
    };
    if what.is_empty() { who } else { format!("{who} · {}", clip(what, 60)) }
}

fn claude_turn(registry: &Registry, emit: &Emit, emit_step: &EmitStep, turn: &Turn, last: &mut Option<Value>) -> anyhow::Result<()> {
    let claude = atalaya_core::worker::find_claude()
        .ok_or_else(|| anyhow::anyhow!("no se encontró Claude Code en esta máquina (comando `claude`)"))?;

    let folders = project_folders(registry)?;
    // Con un proyecto elegido, trabaja en su carpeta. Si no, en la primera carpeta de proyectos.
    let project = match turn.slug {
        Some(slug) => registry.get(slug)?,
        None => None,
    };
    let folder = project
        .as_ref()
        .and_then(|p| p.local_path.clone())
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
        .or_else(|| atalaya_core::discover::roots(registry).ok().and_then(|r| r.into_iter().find(|p| p.is_dir())))
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from));

    let mut prompt = PROMPT.to_string();
    prompt.push_str(MEMORY);
    if turn.mode != "lectura" {
        prompt.push_str(POWERS);
    }
    prompt.push_str(match turn.mode {
        "lectura" => READ_ONLY_PROMPT,
        "autonomo" => AUTONOMOUS,
        _ => ASKING,
    });
    if turn.mode != "lectura" && !folders.is_empty() {
        let list: Vec<String> = folders.iter().map(|(name, path)| format!("{name}: {}", path.display())).collect();
        prompt.push_str(&format!(" Carpetas de los proyectos: {}.", list.join("; ")));
    }
    if let Some(p) = &project {
        prompt.push_str(&format!(" El usuario está viendo el proyecto «{}» (slug `{}`).", p.name, p.slug));
    }
    // «Claude Code coordina»: si el usuario lo encendió, Claude reparte el trabajo con su equipo.
    // En solo lectura solo puede consultar; en los demás modos también encargar cambios.
    let coordinates = match atalaya_core::delegate::brief(registry, turn.mode != "lectura")? {
        Some(brief) => {
            prompt.push_str(&brief);
            true
        }
        None => false,
    };
    let (read_tools, read_only) = if coordinates {
        (format!("{READ_TOOLS},{CONSULT},{CONSULTS}"), format!("{READ_ONLY},{CONSULT},{CONSULTS}"))
    } else {
        (READ_TOOLS.to_string(), READ_ONLY.to_string())
    };
    // Sin coordinar, las herramientas de delegar ni se ofrecen.
    let denied = if coordinates { "AskUserQuestion".to_string() } else { format!("AskUserQuestion,{CONSULT},{WORK},{CONSULTS},{WORKS}") };

    // Las instrucciones del usuario, en el mensaje de sistema; sus skills, como plugin de la sesión.
    prompt.push_str(&atalaya_core::skills::for_ai(registry, turn.slug, CLAUDE_CODE, atalaya_core::skills::Reader::Plugin)?);
    let plugin = atalaya_core::skills::plugin_dir(registry, turn.slug, CLAUDE_CODE)?;

    // Los MCP externos que rigen aquí; sus llaves, solo en el entorno de esta sesión.
    let external = atalaya_core::mcp_servers::for_run(registry, turn.slug, CLAUDE_CODE)?;
    let mcp = atalaya_core::round::mcp_config_plus(&[("ATALAYA_CHAT_CONVERSATION", turn.conversation)], &external)?;
    let message = format!("{}{}", turn.message, atalaya_core::attachments::note(turn.attachments));
    let mut command = Command::new(claude);
    command
        .args(["-p", &message, "--output-format", "stream-json", "--verbose", "--include-partial-messages"])
        .args(["--append-system-prompt", &prompt])
        .args(atalaya_core::ai::claude_cap_args(registry)?)
        // Las herramientas de esta misma versión de Atalaya, sin depender de la configuración del usuario.
        .args(["--mcp-config", &mcp, "--strict-mcp-config"])
        // Una app abierta desde Finder no trae el PATH de la terminal: sin esto no hay npm, git de Homebrew…
        .env("PATH", atalaya_core::worker::developer_path())
        // Cada mensaje es un turno completo: nada queda corriendo en segundo plano después de responder.
        .env("CLAUDE_CODE_DISABLE_BACKGROUND_TASKS", "1")
        .envs(external.env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    match turn.mode {
        "lectura" => {
            command.args(["--tools", "Read,Grep,Glob", "--allowedTools", &read_only, "--permission-mode", "default"]);
            if !coordinates {
                command.args(["--disallowedTools", &denied]);
            }
        }
        "autonomo" => {
            command.args(["--permission-mode", "bypassPermissions", "--disallowedTools", &denied]);
        }
        _ => {
            // Modo explícito: un modo automático en la configuración del usuario aprobaría solo lo que aquí se pregunta.
            // Encargar un cambio a otra IA también espera el visto bueno.
            command
                .args(["--permission-mode", "default", "--allowedTools", &read_tools])
                .args(["--permission-prompt-tool", "mcp__atalaya__aprobar_accion", "--disallowedTools", &denied]);
        }
    }
    if let Some(dir) = &plugin {
        command.arg("--plugin-dir").arg(dir);
    }
    // Los adjuntos se leen sin pedir permiso, también en solo lectura. Van sus carpetas, no
    // solo la de esta conversación: puedes haberlos adjuntado antes de empezar una nueva.
    let mut shared: Vec<PathBuf> = turn.attachments.iter().filter_map(|a| PathBuf::from(&a.path).parent().map(PathBuf::from)).collect();
    shared.dedup();
    for dir in shared {
        command.arg("--add-dir").arg(dir);
    }
    if turn.mode != "lectura" {
        for (_, path) in &folders {
            if Some(path) != folder.as_ref() {
                command.arg("--add-dir").arg(path);
            }
        }
    }
    if let Some(session) = turn.session {
        command.args(["--resume", session]);
    }
    if let Some(folder) = &folder {
        command.current_dir(folder);
    }
    // Grupo propio: detener la conversación detiene también lo que lanzó.
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    let mut child = command.spawn()?;
    RUNNING.lock().unwrap().get_or_insert_with(HashMap::new).insert(turn.conversation.to_string(), child.id());
    let stdout = child.stdout.take().expect("stdout");

    let mut finished = false;
    let mut started: Option<String> = None;
    let mut streamed = false;
    let mut blocks = 0;
    // Las mejoras que este turno movió de etapa.
    let mut staged: Vec<i64> = Vec::new();
    // Los subagentes que lanzó, por la llamada que los lanzó: sus pasos llegan con esa llamada.
    let mut subagents: HashMap<String, String> = HashMap::new();
    for line in BufReader::new(stdout).lines() {
        let Ok(line) = line else { break };
        let Ok(event) = serde_json::from_str::<Value>(&line) else { continue };
        let parent = event["parent_tool_use_id"].as_str().map(String::from);
        let actor = match &parent {
            Some(id) => subagents.get(id).cloned().unwrap_or_else(|| "Subagente".into()),
            None => CLAUDE.to_string(),
        };
        // El texto de un subagente es para Claude Code, no para el usuario: de ellos se ven sus pasos.
        let from_subagent = parent.is_some();
        match event["type"].as_str() {
            // La sesión se conoce desde el comienzo: si se detiene, la conversación sigue desde ahí.
            Some("system") if event["subtype"] == "init" => {
                started = event["session_id"].as_str().map(String::from);
            }
            Some("stream_event") if !from_subagent => {
                let e = &event["event"];
                match e["type"].as_str() {
                    Some("message_start") => streamed = false,
                    Some("content_block_start") if e["content_block"]["type"] == "text" => {
                        // Un bloque de texto nuevo empieza en otro párrafo.
                        if blocks > 0 {
                            emit("delta", "\n\n".into(), None, None);
                        }
                        blocks += 1;
                    }
                    Some("content_block_delta") if e["delta"]["type"] == "text_delta" => {
                        streamed = true;
                        emit("delta", e["delta"]["text"].as_str().unwrap_or_default().to_string(), None, None);
                    }
                    _ => {}
                }
            }
            Some("assistant") => {
                for block in event["message"]["content"].as_array().into_iter().flatten() {
                    match block["type"].as_str() {
                        Some("text") if !streamed && !from_subagent => {
                            emit("texto", block["text"].as_str().unwrap_or_default().to_string(), None, None)
                        }
                        Some("tool_use") => {
                            let name = block["name"].as_str().unwrap_or_default();
                            blocks = 0;
                            if name == STAGE {
                                if let Some(id) = block["input"]["id"].as_i64() {
                                    staged.push(id);
                                }
                            }
                            let id = block["id"].as_str().map(String::from);
                            let spawns = matches!(name, "Agent" | "Task").then(|| subagent_label(&block["input"]));
                            if let (Some(id), Some(label)) = (&id, &spawns) {
                                subagents.insert(id.clone(), label.clone());
                            }
                            emit_step(
                                "herramienta",
                                tool_name(name),
                                tool_detail(name, &block["input"]),
                                Step { id, parent: parent.clone(), actor: actor.clone(), spawns },
                            );
                        }
                        _ => {}
                    }
                }
            }
            Some("user") => {
                for block in event["message"]["content"].as_array().into_iter().flatten() {
                    if block["type"] == "tool_result" {
                        let output = tool_output(&block["content"]);
                        let failed = block["is_error"].as_bool().unwrap_or(false);
                        // Siempre llega, aunque venga vacío: así se sabe que ese paso terminó.
                        let text = if output.trim().is_empty() { String::new() } else { clip(&output, 2000) };
                        let id = block["tool_use_id"].as_str().map(String::from);
                        emit_step(
                            "resultado",
                            text,
                            failed.then(|| "error".into()),
                            Step { id, parent: parent.clone(), actor: actor.clone(), spawns: None },
                        );
                    }
                }
            }
            Some("result") => {
                finished = true;
                let session = event["session_id"].as_str().map(String::from);
                if event["is_error"].as_bool().unwrap_or(false) {
                    let why = atalaya_core::ai::claude_cap_note(&event)
                        .unwrap_or_else(|| event["result"].as_str().unwrap_or("Claude no pudo responder").to_string());
                    emit("error", why, None, session);
                } else {
                    emit("fin", String::new(), None, session);
                }
                *last = Some(event);
            }
            _ => {}
        }
    }
    let output = child.wait_with_output()?;
    let stopped = RUNNING.lock().unwrap().get_or_insert_with(HashMap::new).remove(turn.conversation).is_none();
    // Lo que quedó "en curso" al terminar el turno nadie lo sigue trabajando: vuelve a "aceptada".
    let note = if stopped {
        "Se detuvo la conversación del Agente antes de terminarla. Pídele que siga o trabájala desde aquí."
    } else {
        "Quedó a medias en la conversación del Agente. Pídele que siga o trabájala desde aquí."
    };
    if let Err(e) = registry.release_improvements(&staged, note) {
        eprintln!("no se pudieron liberar las mejoras en curso: {e:#}");
    }
    if !finished {
        if stopped {
            emit("error", "Detenido.".into(), None, started);
            return Ok(());
        }
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        anyhow::bail!(if detail.is_empty() { "Claude terminó sin responder".to_string() } else { detail });
    }
    Ok(())
}

/// Envía un mensaje. La respuesta llega por eventos `chat` con el mismo `request`.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn chat_send(
    app: AppHandle,
    request: String,
    message: String,
    session: Option<String>,
    slug: Option<String>,
    model: Option<String>,
    mode: Option<String>,
    conversation: Option<String>,
    attachments: Option<Vec<String>>,
) -> Result<(), String> {
    // Solo archivos de la carpeta de adjuntos: el mensaje no puede apuntar a otra parte.
    let attachments = atalaya_core::attachments::check(&attachments.unwrap_or_default()).map_err(|e| format!("{e:#}"))?;
    tauri::async_runtime::spawn_blocking(move || {
        let conversation = conversation.unwrap_or_else(|| request.clone());
        // `/nombre …`: un comando guardado se convierte en su mensaje completo.
        let message = Registry::open_default()
            .and_then(|r| atalaya_core::commands::expand(&r, &message))
            .ok()
            .flatten()
            .unwrap_or(message);
        let turn = Turn {
            request: &request,
            message: &message,
            session: session.as_deref(),
            slug: slug.as_deref(),
            mode: mode.as_deref().unwrap_or("preguntar"),
            conversation: &conversation,
            attachments: &attachments,
        };
        if let Err(e) = run(&app, &turn, model.as_deref()) {
            let _ = app.emit(
                "chat",
                ChatEvent {
                    request: request.clone(),
                    kind: "error",
                    text: format!("{e:#}"),
                    detail: None,
                    session: None,
                    step: None,
                },
            );
        }
    })
    .await
    .map_err(|e| e.to_string())
}

/// Detiene la conversación: el proceso de Claude Code, todo lo que lanzó y sus preguntas pendientes.
#[tauri::command]
pub async fn chat_stop(conversation: String) -> Result<(), String> {
    let pid = RUNNING.lock().unwrap().get_or_insert_with(HashMap::new).remove(&conversation);
    if let Some(pid) = pid {
        let _ = Command::new("/bin/kill").args(["-TERM", &format!("-{pid}")]).status();
    }
    tauri::async_runtime::spawn_blocking(move || {
        let registry = Registry::open_default().map_err(|e| e.to_string())?;
        atalaya_core::approvals::cancel(&registry, &conversation).map_err(|e| e.to_string())?;
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Lo que el agente espera que apruebes en esta conversación.
#[tauri::command]
pub async fn chat_pending(conversation: String) -> Result<Vec<atalaya_core::approvals::Approval>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let registry = Registry::open_default().map_err(|e| e.to_string())?;
        atalaya_core::approvals::pending(&registry, &conversation).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Lo que van haciendo las IA a las que Claude Code les encargó algo en esta conversación,
/// después del último `id` visto.
#[tauri::command]
pub async fn chat_activity(conversation: String, after: i64) -> Result<Vec<atalaya_core::activity::Entry>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let registry = Registry::open_default().map_err(|e| e.to_string())?;
        atalaya_core::activity::since(&registry, &conversation, after).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Tu respuesta a una acción: permitir (una vez o siempre en esta conversación) o rechazar.
#[tauri::command]
pub async fn chat_answer(id: i64, allow: bool, remember: bool, message: Option<String>) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let registry = Registry::open_default().map_err(|e| e.to_string())?;
        atalaya_core::approvals::answer(&registry, id, allow, remember, message.as_deref()).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Un adjunto como lo muestra el chat: con su vista previa si es una imagen.
#[derive(Serialize)]
pub struct AttachmentView {
    #[serde(flatten)]
    file: atalaya_core::attachments::Attachment,
    preview: Option<String>,
}

fn view(file: atalaya_core::attachments::Attachment) -> AttachmentView {
    let preview = atalaya_core::attachments::preview(&file);
    AttachmentView { file, preview }
}

/// Guarda lo que pegaste en la caja del chat (llega en base64).
#[tauri::command]
pub async fn chat_attach_data(conversation: String, name: String, data: String) -> Result<AttachmentView, String> {
    tauri::async_runtime::spawn_blocking(move || {
        use base64::Engine;
        let bytes = base64::engine::general_purpose::STANDARD.decode(data.as_bytes())?;
        Ok(view(atalaya_core::attachments::save(&conversation, &name, &bytes)?))
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e: anyhow::Error| format!("{e:#}"))
}

/// Copia a la conversación los archivos que arrastraste o elegiste.
#[tauri::command]
pub async fn chat_attach_paths(conversation: String, paths: Vec<String>) -> Result<Vec<AttachmentView>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        paths
            .iter()
            .map(|p| atalaya_core::attachments::copy(&conversation, std::path::Path::new(p)).map(view))
            .collect::<anyhow::Result<Vec<_>>>()
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| format!("{e:#}"))
}
