//! La ronda del agente: el validador revisa el estado de todos los proyectos y deja
//! propuestas en la cola de Decisiones. Solo lee y propone; no ejecuta nada.
//!
//! El validador es el rol de Ajustes → IA: Claude Code (con las herramientas del MCP)
//! o cualquier modelo por API (con el paquete de contexto en JSON).

use crate::Registry;
use crate::connections::Connection;
use crate::orchestrator::Candidate;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

const TOOLS: &str = "mcp__atalaya__list_projects,mcp__atalaya__project_status,mcp__atalaya__get_project_context,\
mcp__atalaya__list_improvements,mcp__atalaya__list_decisions,mcp__atalaya__propose_decision,\
mcp__atalaya__github_issues,mcp__atalaya__radar_events,mcp__atalaya__claude_usage";

const TASK: &str = "Haz la ronda de Atalaya. Revisa el estado de todos los proyectos con las herramientas y deja \
en la cola de Decisiones lo que de verdad necesita que el usuario decida algo. Pasos: \
1) list_projects y project_status para ver alertas; list_improvements y list_decisions para no repetir lo que ya \
existe; github_issues en los proyectos con repo; radar_events para cambios de la competencia. \
2) Por cada asunto que merezca acción, llama a propose_decision con: un título corto y concreto; en `context` la \
evidencia tal como la viste (qué revisión, qué issue, qué cambio); en `proposal` lo que propones, redactado para \
que se pueda aprobar tal cual; y la acción: `mejora` siempre que un agente pueda resolverlo o avanzar trabajando en el repositorio del proyecto \
(el texto de `proposal` será el encargo para ese agente, así que descríbelo completo; es la acción preferida), `comentario` para responder un issue de \
GitHub (con `target` = número del issue y `proposal` = el comentario listo para publicar), `reconocer` para una \
alerta que es ruido conocido (con `target` = tipo de revisión), `actualizar` cuando la carpeta local del proyecto \
quedó atrasada respecto de su rama principal (los agentes trabajan en una copia aparte y no pueden tocar esa carpeta: \
nunca lo propongas como `mejora`; Atalaya la pone al día sola), o `ninguna` solo cuando nada se puede hacer desde el código y basta con que el usuario lo sepa. \
Reglas: máximo 5 propuestas, las más importantes primero. No propongas nada que ya tenga una mejora abierta o una \
decisión pendiente. No propongas por proponer: si todo está en orden, no crees ninguna. No inventes datos. \
3) Termina con un resumen en español de 3 a 6 líneas, en texto simple: lo más importante de la ronda y cuántas \
decisiones dejaste.";

/// La configuración MCP de esta misma versión de Atalaya, para pasarla a Claude Code:
/// así nadie tiene que configurar el MCP a mano.
pub fn mcp_config() -> Result<String> {
    mcp_config_with(&[])
}

/// Igual, con variables de entorno para el servidor MCP. La base y el Llavero en uso
/// pasan siempre, para que el MCP vea lo mismo que quien lo lanza.
pub fn mcp_config_with(env: &[(&str, &str)]) -> Result<String> {
    mcp_config_plus(env, &crate::mcp_servers::Delivery::default())
}

/// Igual, más los MCP externos de esa sesión. Sus llaves van como `${VARIABLE}`: quien lanza
/// a Claude Code pone `extra.env` en el entorno de esa sesión, nunca en la línea de comando.
pub fn mcp_config_plus(env: &[(&str, &str)], extra: &crate::mcp_servers::Delivery) -> Result<String> {
    let exe = std::env::current_exe()?;
    let mut vars = serde_json::Map::new();
    for key in ["ATALAYA_DB", "ATALAYA_KEYCHAIN_SERVICE"] {
        if let Ok(value) = std::env::var(key) {
            vars.insert(key.into(), value.into());
        }
    }
    for (k, v) in env {
        vars.insert((*k).into(), (*v).into());
    }
    // Un encargo a otra IA puede durar media hora: el límite de Claude Code para esta herramienta
    // (y su espera sin novedades) queda en 45 minutos.
    let mut servers = serde_json::Map::new();
    servers.insert("atalaya".into(), json!({ "command": mcp_command(&exe), "args": ["mcp"], "env": vars, "timeout": 2_700_000 }));
    for s in &extra.servers {
        servers.insert(s.name.clone(), crate::mcp_servers::entry(s, &extra.env, false));
    }
    Ok(json!({ "mcpServers": servers }).to_string())
}

const API_TASK: &str = "Haz la ronda de Atalaya con el estado de los proyectos que va abajo en JSON (revisiones, \
mejoras abiertas, decisiones pendientes, eventos recientes e issues de GitHub). Deja en la cola de Decisiones solo lo \
que de verdad necesita que el usuario decida algo. Por cada asunto: un título corto y concreto; en `context` la \
evidencia tal como aparece en los datos; en `proposal` lo que propones, redactado para aprobarse tal cual; y la \
acción: `mejora` cuando un agente pueda resolverlo trabajando en el repositorio (el texto de `proposal` será su \
encargo; es la acción preferida), `comentario` para responder un issue (con `target` = número del issue y \
`proposal` = el comentario), `reconocer` para una alerta que es ruido conocido (con `target` = tipo de revisión), \
`actualizar` cuando la carpeta local quedó atrasada respecto de su rama principal (nunca como `mejora`: los agentes \
no pueden tocar esa carpeta) o `ninguna`. Reglas: máximo 5, las más importantes primero; nada que ya tenga una mejora abierta o una decisión \
pendiente; si todo está en orden, ninguna; no inventes datos. Responde SOLO con un objeto JSON: \
{\"decisiones\": [{\"slug\", \"kind\" (alerta, issue o mantenimiento), \"title\", \"context\", \"proposal\", \
\"action\", \"target\" (o null)}], \"resumen\": \"3 a 6 líneas en español, texto simple\"}.";

/// La ronda con un modelo por API o un agente de terminal: recibe todo el contexto de una
/// vez y responde en JSON.
fn run_api(registry: &Registry, c: &Connection, emit: &dyn Fn(&str, &str)) -> Result<String> {
    emit("paso", "Arma el contexto de los proyectos");
    let mut portfolio = crate::context::portfolio(registry)?;
    for project in portfolio["projects"].as_array_mut().into_iter().flatten() {
        let Some(repo) = project["project"]["repo"].as_str().map(String::from) else { continue };
        if let Ok(issues) = crate::github::issues(&repo) {
            let list: Vec<Value> = issues.as_array().into_iter().flatten().take(8).cloned().collect();
            project["github_issues"] = Value::Array(list);
        }
    }
    emit("paso", &format!("{} analiza", c.name));
    let reply = crate::orchestrator::ask(
        registry,
        &Candidate::Connection(c.clone()),
        "Eres el validador de Atalaya, la consola que vigila los proyectos del usuario.",
        &format!("{API_TASK}\n\n{portfolio}"),
        "ronda",
        None,
    )?;
    let parsed = crate::orchestrator::extract_json(&reply).context("el modelo no respondió con el JSON pedido")?;
    let mut created = 0;
    let mut skipped = Vec::new();
    for d in parsed["decisiones"].as_array().into_iter().flatten().take(5) {
        let field = |k: &str| d[k].as_str().unwrap_or_default().to_string();
        let target = d["target"].as_str().map(String::from).or_else(|| d["target"].as_i64().map(|n| n.to_string()));
        let kind = Some(field("kind")).filter(|k| !k.is_empty()).unwrap_or_else(|| "mantenimiento".into());
        match crate::decisions::propose(
            registry,
            &field("slug"),
            &kind,
            &field("title"),
            &field("context"),
            &field("proposal"),
            &field("action"),
            target.as_deref(),
        ) {
            Ok(_) => {
                created += 1;
                emit("paso", &format!("propose_decision: {}", field("title")));
            }
            Err(e) => skipped.push(format!("«{}»: {e:#}", field("title"))),
        }
    }
    let mut summary = parsed["resumen"].as_str().unwrap_or_default().trim().to_string();
    if summary.is_empty() {
        summary = format!("{created} decisiones nuevas.");
    }
    if !skipped.is_empty() {
        summary.push_str(&format!("\n\nNo se guardaron: {}", skipped.join(" · ")));
    }
    Ok(summary)
}

/// Ejecuta la ronda ahora. `emit(tipo, texto)` recibe el avance. Devuelve el resumen del validador.
pub fn run(registry: &Registry, emit: &dyn Fn(&str, &str)) -> Result<String> {
    let before = crate::decisions::list(registry, true)?.len();
    let (summary, by) = crate::orchestrator::attempt(registry, "validador", &[], |who| {
        emit("estado", &format!("{} hace la ronda", who.label()));
        match who {
            Candidate::ClaudeCode => crate::ai::tracked_claude(registry, "ronda", None, |last| run_claude(registry, emit, last)),
            Candidate::Connection(c) => run_api(registry, c, emit),
        }
    })?;
    finish(registry, before, summary, &by.label())
}

fn run_claude(registry: &Registry, emit: &dyn Fn(&str, &str), last: &mut Option<Value>) -> Result<String> {
    let claude = crate::worker::find_claude().context("no se encontró Claude Code en esta máquina (comando `claude`)")?;
    // El agente usa las herramientas de esta misma versión de Atalaya, no las de otra instalada.
    let mcp = mcp_config()?;
    // Sin herramientas de archivos: las skills van como texto, junto a las instrucciones.
    let guidance = crate::skills::for_ai(registry, None, crate::orchestrator::CLAUDE_CODE, crate::skills::Reader::Inline)?;

    let mut child = Command::new(claude)
        .args(["-p", TASK, "--output-format", "stream-json", "--verbose"])
        .args(["--append-system-prompt", &format!("Responde en español.{guidance}")])
        .args(["--tools", "", "--allowedTools", TOOLS])
        .args(crate::ai::claude_cap_args(registry)?)
        .args(["--mcp-config", &mcp, "--strict-mcp-config", "--no-session-persistence"])
        .env("PATH", crate::worker::developer_path())
        .current_dir(Registry::default_path()?.parent().context("sin carpeta de datos")?)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = child.stdout.take().expect("stdout");
    let mut summary = None;
    for line in BufReader::new(stdout).lines() {
        let Ok(event) = serde_json::from_str::<Value>(&line?) else { continue };
        match event["type"].as_str() {
            Some("assistant") => {
                for block in event["message"]["content"].as_array().into_iter().flatten() {
                    if block["type"] == "tool_use" {
                        let name = block["name"].as_str().unwrap_or_default().trim_start_matches("mcp__atalaya__");
                        emit("paso", name);
                    }
                }
            }
            Some("result") => {
                let text = event["result"].as_str().unwrap_or_default().to_string();
                let failed = event["is_error"].as_bool().unwrap_or(false);
                let text = crate::ai::claude_cap_note(&event).unwrap_or(text);
                *last = Some(event);
                if failed {
                    bail!("Claude no pudo terminar la ronda: {text}");
                }
                summary = Some(text);
            }
            _ => {}
        }
    }
    let output = child.wait_with_output()?;
    summary.with_context(|| {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if detail.is_empty() { "Claude terminó sin responder".to_string() } else { detail }
    })
}

fn finish(registry: &Registry, before: usize, summary: String, by: &str) -> Result<String> {
    let (today, _) = registry.local_now()?;
    let created = crate::decisions::list(registry, true)?.len().saturating_sub(before);
    registry.set_setting("round_last_date", &today)?;
    registry.set_setting("round_last_at", &registry.utc_now()?)?;
    registry.set_setting("round_summary", &summary)?;
    registry.set_setting("round_by", by)?;
    if created > 0 {
        let title = format!("Ronda del agente: {created} {} por revisar", if created == 1 { "decisión" } else { "decisiones" });
        crate::checks::notify(registry, "decisiones", &title, summary.lines().next().unwrap_or_default());
    }
    Ok(summary)
}

/// La app de escritorio no sirve MCP: en ese caso se usa el comando `atalaya` instalado junto a ella.
/// `ATALAYA_CLI` elige otro, para probar una versión sin instalarla.
/// La CLI de Atalaya que se lanza como servidor MCP (o como su puente).
pub fn mcp_command(exe: &std::path::Path) -> String {
    if let Some(cli) = std::env::var_os("ATALAYA_CLI").filter(|c| !c.is_empty()) {
        return std::path::PathBuf::from(cli).display().to_string();
    }
    let is_cli = exe.file_name().is_some_and(|n| n == "atalaya");
    if is_cli {
        return exe.display().to_string();
    }
    ["/usr/local/bin/atalaya"]
        .into_iter()
        .map(std::path::PathBuf::from)
        .chain(dirs::home_dir().map(|h| h.join(".cargo/bin/atalaya")))
        .find(|p| p.is_file())
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "atalaya".into())
}

/// Lo último que dejó la ronda: (fecha y hora UTC, resumen).
pub fn last(registry: &Registry) -> Result<Value> {
    Ok(json!({
        "at": registry.setting("round_last_at")?,
        "summary": registry.setting("round_summary")?,
        "by": registry.setting("round_by")?,
        "auto": registry.setting("round_auto")?.as_deref() != Some("no"),
    }))
}

/// Ronda del vigía: una vez al día, a la hora del resumen, salvo que se haya desactivado.
pub fn daily(registry: &Registry) -> Result<bool> {
    if registry.setting("round_auto")?.as_deref() == Some("no")
        || crate::orchestrator::lineup(registry, "validador", &[])?.0.is_empty()
    {
        return Ok(false);
    }
    let (today, hour) = registry.local_now()?;
    let wanted: u32 = registry
        .setting(crate::mail::REPORT_HOUR)?
        .and_then(|h| h.parse().ok())
        .unwrap_or(8);
    // Una hora antes del resumen diario, para que el correo ya lleve las decisiones.
    if hour + 1 < wanted || registry.setting("round_last_date")?.as_deref() == Some(today.as_str()) {
        return Ok(false);
    }
    // Se marca antes de correr: si falla, no reintenta cada minuto.
    registry.set_setting("round_last_date", &today)?;
    run(registry, &|_, _| {})?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestrator::{RoleConfig, set_config};
    use crate::testutil;

    #[test]
    fn an_api_model_can_be_the_validator() {
        let r = testutil::registry("ronda");
        testutil::project(&r, "tienda");
        let answer = json!({
            "decisiones": [
                { "slug": "tienda", "kind": "alerta", "title": "Renovar certificado", "context": "vence en 3 días",
                  "proposal": "Renueva el certificado del dominio", "action": "mejora", "target": null },
                { "slug": "no-existe", "title": "Otra", "context": "x", "proposal": "y", "action": "ninguna" }
            ],
            "resumen": "Una alerta importante."
        });
        let url = testutil::chat_server(format!("```json\n{answer}\n```"));
        let c = testutil::local_ai(&r, "Local", &url, "llama3");
        set_config(&r, "validador", &RoleConfig { candidates: vec![c.id.clone()], strategy: "primero".into() }).unwrap();

        let summary = run(&r, &|_, _| {}).unwrap();
        assert!(summary.starts_with("Una alerta importante."), "{summary}");
        assert!(summary.contains("no-existe"), "debe contar la que no se guardó: {summary}");
        let pending = crate::decisions::list(&r, true).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].title, "Renovar certificado");
        assert_eq!(last(&r).unwrap()["by"], "Local (llama3)");
        // La llamada quedó anotada en el consumo como tarea de ronda.
        let tasks: i64 = r.conn().query_row("SELECT count(*) FROM ai_usage WHERE task = 'ronda'", [], |x| x.get(0)).unwrap();
        assert_eq!(tasks, 1);
    }
}
