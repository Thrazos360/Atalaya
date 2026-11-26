//! Claude Code coordina: desde el chat le encarga trabajo a las otras IA que la persona eligió
//! en sus roles, para repartir el trabajo y gastar menos de Claude.
//!
//! Lo que elige la persona manda:
//! - se enciende en IA → «Claude Code coordina», y viene apagado;
//! - solo se delega en quienes están en algún rol, nunca en otra IA conectada;
//! - si Claude no nombra a nadie, se sigue el orden y la estrategia del rol.
//!
//! Las consultas solo leen. Los trabajos los hace un agente de terminal dentro del encierro de
//! macOS, escribiendo solo en la carpeta del proyecto. Todo queda anotado como «delegado».

use crate::Registry;
use crate::orchestrator::{self, Candidate};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::Duration;

pub const SETTING: &str = "ai_coordinate";

/// Cuánto puede trabajar un agente en un encargo.
const WORK_TIMEOUT: Duration = Duration::from_secs(30 * 60);

const SYSTEM: &str = "Trabajas para Claude Code, que coordina el mantenimiento de los proyectos del usuario en \
Atalaya y te encarga una parte. Responde en español, concreto y verificable: qué encontraste, dónde (archivo y línea) \
y qué recomiendas. No inventes: si no pudiste comprobar algo, dilo.";

const WORK_RULES: &str = "Trabajas para Claude Code, que coordina el mantenimiento de este proyecto y te encarga \
este cambio. Haz solo lo que pide el encargo, en esta carpeta. No subas nada (git push), no despliegues y no borres \
archivos que no creaste. Al terminar, resume en español qué archivos cambiaste y cómo lo comprobaste.";

pub fn enabled(registry: &Registry) -> Result<bool> {
    Ok(registry.setting(SETTING)?.as_deref() == Some("si"))
}

pub fn set_enabled(registry: &Registry, on: bool) -> Result<()> {
    registry.set_setting(SETTING, if on { "si" } else { "no" })
}

/// Las IA de los roles a las que Claude Code puede encargar trabajo: todas menos él mismo,
/// con sus roles, si son gratis, si pueden cambiar archivos y si están disponibles.
pub fn team(registry: &Registry) -> Result<Vec<Value>> {
    let mut members: Vec<(Candidate, Vec<&str>)> = Vec::new();
    for (role, _, _) in orchestrator::ROLES {
        for id in orchestrator::config(registry, role)?.candidates {
            let Some(candidate) = orchestrator::resolve(registry, &id)? else { continue };
            if matches!(candidate, Candidate::ClaudeCode) {
                continue;
            }
            match members.iter_mut().find(|(c, _)| c.id() == candidate.id()) {
                Some((_, roles)) => roles.push(role),
                None => members.push((candidate, vec![role])),
            }
        }
    }
    members
        .into_iter()
        .map(|(c, roles)| {
            Ok(json!({
                "id": c.id(),
                "name": c.label(),
                "roles": roles,
                "free": orchestrator::is_free(&c),
                "can_edit": c.is_agent(),
                "unavailable": orchestrator::unavailable(registry, &c)?,
            }))
        })
        .collect()
}

/// Para la app: si está encendido y con quién cuenta.
pub fn summary(registry: &Registry) -> Result<Value> {
    Ok(json!({ "enabled": enabled(registry)?, "team": team(registry)? }))
}

/// Lo que Claude Code necesita saber para coordinar, para su instrucción de sistema. `None`
/// si está apagado o no hay nadie disponible a quien encargar.
pub fn brief(registry: &Registry, can_work: bool) -> Result<Option<String>> {
    if !enabled(registry)? {
        return Ok(None);
    }
    let team = team(registry)?;
    let ready: Vec<&Value> = team.iter().filter(|m| m["unavailable"].is_null()).collect();
    if ready.is_empty() {
        return Ok(None);
    }
    let mut text = String::from(
        " Coordinas un equipo de IA que el usuario eligió en sus roles de Atalaya. Para repartir el trabajo y ahorrar el \
tuyo, encárgales lo que puedan hacer bien: con delegar_consulta, análisis, revisiones, resúmenes o búsquedas en el \
código (solo leen)",
    );
    if can_work {
        text.push_str("; con delegar_trabajo, cambios acotados en la carpeta de un proyecto (solo los que pueden cambiar archivos)");
    }
    text.push_str(
        ". Si son varias cosas independientes, repártelas a la vez en una sola llamada: delegar_consultas para varias \
consultas",
    );
    if can_work {
        text.push_str(" y delegar_trabajos para varios cambios, uno por proyecto");
    }
    text.push_str(&format!(
        " (hasta {MAX_ORDERS}). Sin ia nombrada se reparten entre los del rol; nombra a cada uno si sabes quién sirve mejor"
    ));
    text.push_str(
        ". Tú decides qué encargar y a quién, revisas lo que devuelven antes de darlo por bueno (con git diff si \
cambiaron archivos) y le respondes al usuario. Prefiere a los gratis. Lo delicado, lo que necesita criterio o lo que \
toca producción hazlo tú. Cuéntale al usuario a quién le encargaste qué. Equipo:",
    );
    for m in ready {
        let roles: Vec<&str> = m["roles"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
        text.push_str(&format!(
            "\n- {} (ia: \"{}\"): {}, {}; roles: {}.",
            m["name"].as_str().unwrap_or_default(),
            m["id"].as_str().unwrap_or_default(),
            if m["free"] == true { "gratis" } else { "de pago o con su cuenta" },
            if m["can_edit"] == true { "puede cambiar archivos" } else { "solo responde" },
            roles.join(", ")
        ));
    }
    Ok(Some(text))
}

/// Una pregunta a otra IA del equipo: lee y responde, sin cambiar nada. Sin `ai`, la elige
/// el rol (por defecto el validador, que es quien analiza).
///
/// Con `conversation` (la del chat que lo pidió), su avance queda en `activity` para verlo en vivo.
pub fn consult(
    registry: &Registry,
    ai: Option<&str>,
    role: Option<&str>,
    task: &str,
    slug: Option<&str>,
    conversation: Option<&str>,
) -> Result<String> {
    ensure_enabled(registry)?;
    consult_one(registry, ai, role, task, slug, conversation, None)
}

fn consult_one(
    registry: &Registry,
    ai: Option<&str>,
    role: Option<&str>,
    task: &str,
    slug: Option<&str>,
    conversation: Option<&str>,
    spread: Option<usize>,
) -> Result<String> {
    let folder = folder_of(registry, slug)?;
    let (text, who) = with_member(registry, ai, role.unwrap_or("validador"), false, spread, |c| {
        let prompt = with_project(registry, c, task, slug, folder.as_deref())?;
        live(registry, conversation, c, &format!("Consulta: {task}"), || orchestrator::ask(registry, c, SYSTEM, &prompt, "delegado", slug))
    })?;
    Ok(format!("Respondió {}:\n\n{text}", who.label()))
}

/// La pregunta con lo que hace falta del proyecto: un agente lo lee en su carpeta; un modelo
/// por API (o Claude Code sin herramientas) recibe su estado según Atalaya.
pub(crate) fn with_project(
    registry: &Registry,
    c: &Candidate,
    task: &str,
    slug: Option<&str>,
    folder: Option<&std::path::Path>,
) -> Result<String> {
    let mut prompt = task.to_string();
    if let Some(slug) = slug {
        match (folder, c.is_agent()) {
            (Some(f), true) => {
                prompt.push_str(&format!("\n\nEl proyecto «{slug}» está en {}: léelo ahí. No cambies archivos.", f.display()))
            }
            _ => {
                let context = crate::context::pack(registry, slug)?.to_string();
                prompt.push_str(&format!("\n\nEstado del proyecto «{slug}» según Atalaya (JSON):\n{}", clip(&context, 30_000)));
            }
        }
    }
    Ok(prompt)
}

pub(crate) fn folder_of_project(registry: &Registry, slug: Option<&str>) -> Result<Option<PathBuf>> {
    folder_of(registry, slug)
}

/// Un cambio que hace otro agente del equipo en la carpeta del proyecto. Sin `ai`, lo hace el
/// constructor de los roles.
pub fn work(registry: &Registry, ai: Option<&str>, task: &str, slug: &str, conversation: Option<&str>) -> Result<String> {
    ensure_enabled(registry)?;
    work_one(registry, ai, task, slug, conversation, None)
}

fn work_one(
    registry: &Registry,
    ai: Option<&str>,
    task: &str,
    slug: &str,
    conversation: Option<&str>,
    spread: Option<usize>,
) -> Result<String> {
    let folder = folder_of(registry, Some(slug))?.with_context(|| format!("el proyecto «{slug}» no tiene su carpeta en este Mac"))?;
    let (text, who) = with_member(registry, ai, "constructor", true, spread, |c| {
        let Candidate::Connection(conn) = c else { bail!("Claude Code no se encarga trabajo a sí mismo") };
        let guidance = crate::skills::for_ai(registry, Some(slug), &conn.id, crate::skills::Reader::Files)?;
        let prompt = format!("{WORK_RULES}{guidance}\n\n{task}");
        live(registry, conversation, c, &format!("Encargo en {slug}: {task}"), || {
            let run = crate::agents::run_delegated(registry, conn, &folder, &prompt, Some(slug), WORK_TIMEOUT)?;
            if run.timed_out {
                bail!("{} no terminó a tiempo", conn.name);
            }
            if !run.ok {
                bail!("{} no pudo terminar: {}", conn.name, crate::clis::explain_failure(&run.output));
            }
            let text = crate::clis::clean_output(&conn.provider, run.stdout.trim()).trim().to_string();
            Ok(clip_end(if text.is_empty() { &run.output } else { &text }, 8_000))
        })
    })?;
    Ok(format!(
        "{} trabajó en {} y dijo:\n\n{text}\n\nRevisa sus cambios con git diff en esa carpeta antes de darlos por buenos.",
        who.label(),
        folder.display()
    ))
}

/// Hace el encargo dejando en vivo quién lo hace, lo que va diciendo y cómo terminó.
fn live(registry: &Registry, conversation: Option<&str>, c: &Candidate, what: &str, f: impl FnOnce() -> Result<String>) -> Result<String> {
    let recorder = crate::activity::Recorder::start(registry, conversation, &c.label(), &clip(what, 300));
    let result = crate::agents::with_progress(recorder.sink(), f);
    match &result {
        Ok(text) => recorder.finish(true, &clip(text.trim(), 300)),
        Err(e) => recorder.finish(false, &clip(&format!("{e:#}"), 300)),
    }
    result
}

/// Cuántos encargos se pueden repartir de una vez.
pub const MAX_ORDERS: usize = 6;

/// Un encargo de un reparto: qué, a quién (si no, lo reparte el rol) y en qué proyecto.
#[derive(Debug, Clone, Default)]
pub struct Order {
    pub task: String,
    pub ai: Option<String>,
    pub role: Option<String>,
    pub slug: Option<String>,
}

/// Varias consultas a la vez, cada una a una IA del equipo. Las que no nombran a nadie se
/// reparten entre las disponibles del rol, en vez de caer todas en la primera. Si una falla,
/// las demás siguen. Devuelve todas las respuestas juntas, en el orden de los encargos.
pub fn consult_many(registry: &Registry, orders: &[Order], conversation: Option<&str>) -> Result<String> {
    ensure_enabled(registry)?;
    check_orders(orders)?;
    fan_out(registry, orders, conversation, |r, o, spread, conversation| {
        consult_one(r, o.ai.as_deref(), o.role.as_deref(), &o.task, o.slug.as_deref(), conversation, spread)
    })
}

/// Varios cambios a la vez, cada uno en su proyecto: dos agentes en la misma carpeta se
/// pisarían, así que cada proyecto lleva un solo encargo.
pub fn work_many(registry: &Registry, orders: &[Order], conversation: Option<&str>) -> Result<String> {
    ensure_enabled(registry)?;
    check_orders(orders)?;
    let mut seen = std::collections::HashSet::new();
    for o in orders {
        let slug = o.slug.as_deref().filter(|s| !s.is_empty()).context("cada cambio necesita su proyecto")?;
        if !seen.insert(slug) {
            bail!("hay dos cambios para «{slug}»: júntalos en un solo encargo, porque dos agentes en la misma carpeta se pisan");
        }
    }
    fan_out(registry, orders, conversation, |r, o, spread, conversation| {
        work_one(r, o.ai.as_deref(), &o.task, o.slug.as_deref().unwrap_or_default(), conversation, spread)
    })
}

fn check_orders(orders: &[Order]) -> Result<()> {
    if orders.is_empty() {
        bail!("no hay encargos");
    }
    if orders.len() > MAX_ORDERS {
        bail!("son {} encargos; reparte como máximo {MAX_ORDERS} a la vez", orders.len());
    }
    if let Some(i) = orders.iter().position(|o| o.task.trim().is_empty()) {
        bail!("el encargo {} no dice qué hacer", i + 1);
    }
    Ok(())
}

/// Corre cada encargo en su hilo, con su propia conexión a la base, y junta lo que vuelve.
fn fan_out(
    registry: &Registry,
    orders: &[Order],
    conversation: Option<&str>,
    run: impl Fn(&Registry, &Order, Option<usize>, Option<&str>) -> Result<String> + Sync,
) -> Result<String> {
    let path = registry.path();
    // El turno de cada encargo sin IA nombrada: el primero va al primero del rol, el segundo al segundo…
    let mut turn = 0;
    let spreads: Vec<Option<usize>> = orders
        .iter()
        .map(|o| {
            if o.ai.as_deref().is_some_and(|a| !a.trim().is_empty()) {
                None
            } else {
                turn += 1;
                Some(turn - 1)
            }
        })
        .collect();
    let results: Vec<Result<String>> = std::thread::scope(|scope| {
        let handles: Vec<_> = orders
            .iter()
            .zip(&spreads)
            .map(|(order, spread)| {
                let (path, run) = (&path, &run);
                scope.spawn(move || {
                    let registry = Registry::open(path)?;
                    run(&registry, order, *spread, conversation)
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap_or_else(|_| Err(anyhow::anyhow!("el encargo se cayó")))).collect()
    });
    let failed = results.iter().filter(|r| r.is_err()).count();
    let mut text = format!(
        "{} encargos en paralelo: {} bien{}.",
        orders.len(),
        orders.len() - failed,
        if failed > 0 { format!(", {failed} no se pudieron") } else { String::new() }
    );
    for (i, (order, result)) in orders.iter().zip(results).enumerate() {
        text.push_str(&format!("\n\n### Encargo {}: «{}»\n", i + 1, clip(order.task.trim(), 120)));
        match result {
            Ok(answer) => text.push_str(&answer),
            Err(e) => text.push_str(&format!("No se pudo: {e:#}")),
        }
    }
    Ok(text)
}

fn ensure_enabled(registry: &Registry) -> Result<()> {
    if !enabled(registry)? {
        bail!("la coordinación está apagada: el usuario la enciende en Atalaya → IA → «Claude Code coordina»");
    }
    Ok(())
}

fn folder_of(registry: &Registry, slug: Option<&str>) -> Result<Option<PathBuf>> {
    let Some(slug) = slug else { return Ok(None) };
    let project = registry.get(slug)?.with_context(|| format!("no existe el proyecto «{slug}»"))?;
    Ok(project.local_path.map(PathBuf::from).filter(|p| p.is_dir()))
}

/// Hace el trabajo con la IA pedida, si está en el equipo, o con la que diga el rol.
fn with_member<T>(
    registry: &Registry,
    ai: Option<&str>,
    role: &str,
    needs_agent: bool,
    spread: Option<usize>,
    mut work: impl FnMut(&Candidate) -> Result<T>,
) -> Result<(T, Candidate)> {
    let check = |c: &Candidate| -> Result<()> {
        if needs_agent && !c.is_agent() {
            bail!("{} solo responde; para cambiar archivos hace falta un agente", c.label());
        }
        Ok(())
    };
    if let Some(wanted) = ai.map(str::trim).filter(|a| !a.is_empty()) {
        let team = team(registry)?;
        let names: Vec<&str> = team.iter().filter_map(|m| m["id"].as_str()).collect();
        let member = team
            .iter()
            .find(|m| m["id"] == wanted || m["name"].as_str().is_some_and(|n| n.eq_ignore_ascii_case(wanted)))
            .with_context(|| format!("«{wanted}» no está en los roles del usuario; el equipo es: {}", names.join(", ")))?;
        if let Some(why) = member["unavailable"].as_str() {
            bail!("{} no está disponible: {why}", member["name"].as_str().unwrap_or(wanted));
        }
        let id = member["id"].as_str().unwrap_or_default();
        let c = orchestrator::resolve(registry, id)?.with_context(|| format!("la conexión «{id}» ya no existe"))?;
        check(&c)?;
        return Ok((work(&c)?, c));
    }
    let (ready, skipped) = orchestrator::lineup(registry, role, &[Candidate::ClaudeCode])?;
    if ready.is_empty() {
        bail!(
            "el rol {role} no tiene otra IA disponible además de Claude Code{}; el usuario las agrega en Atalaya → IA",
            if skipped.is_empty() { String::new() } else { format!(" ({})", skipped.join(" · ")) }
        );
    }
    let Some(turn) = spread else {
        return orchestrator::attempt(registry, role, &[Candidate::ClaudeCode], |c| {
            check(c)?;
            work(c)
        });
    };
    // En un reparto, cada encargo empieza por otro del rol; si ese falla, siguen los demás.
    let fit: Vec<Candidate> = ready.into_iter().filter(|c| !needs_agent || c.is_agent()).collect();
    if fit.is_empty() {
        bail!("el rol {role} no tiene un agente disponible que pueda cambiar archivos");
    }
    let start = turn % fit.len();
    let mut notes = Vec::new();
    for c in fit[start..].iter().chain(&fit[..start]) {
        match work(c) {
            Ok(value) => return Ok((value, c.clone())),
            Err(e) => notes.push(format!("{}: {e:#}", c.label())),
        }
    }
    bail!("nadie pudo hacer el encargo de {role}. {}", notes.join(" · "))
}

fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max { text.to_string() } else { format!("{}…", text.chars().take(max).collect::<String>()) }
}

fn clip_end(text: &str, max: usize) -> String {
    let n = text.chars().count();
    if n <= max { text.to_string() } else { format!("…{}", text.chars().skip(n - max).collect::<String>()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connections::{Input, save};
    use crate::orchestrator::{RoleConfig, set_config};

    fn registry(name: &str) -> (Registry, PathBuf) {
        let dir = std::env::temp_dir().join(format!("atalaya-delegar-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        (Registry::open(&dir.join("a.db")).unwrap(), dir)
    }

    /// Un agente falso que responde lo que recibe y deja un archivo en su carpeta.
    fn fake_agent(r: &Registry, dir: &std::path::Path) -> String {
        let script = dir.join("agente.sh");
        std::fs::write(&script, "#!/bin/sh\necho \"hecho\" > hecho.txt\necho \"Listo: $(printf '%s' \"$1\" | tail -1)\"\n").unwrap();
        std::process::Command::new("chmod").arg("+x").arg(&script).status().unwrap();
        let command = format!("{} {{tarea}}", script.display());
        save(
            r,
            &Input {
                id: None,
                provider: "agente-personalizado".into(),
                name: "Falso".into(),
                config: json!({ "command": command, "ask_command": command }),
                secret: None,
            },
        )
        .unwrap()
        .id
    }

    #[test]
    fn only_works_when_on_and_only_with_the_chosen_team() {
        let (r, dir) = registry("equipo");
        let id = fake_agent(&r, &dir);
        let project = dir.join("proyecto");
        std::fs::create_dir_all(&project).unwrap();
        crate::discover::add_local(&r, &project, Default::default()).unwrap();
        let slug = r.list().unwrap()[0].slug.clone();

        // Apagado: no delega ni le cuenta nada a Claude.
        assert!(consult(&r, None, None, "revisa", None, None).unwrap_err().to_string().contains("apagada"));
        set_enabled(&r, true).unwrap();
        assert_eq!(brief(&r, true).unwrap(), None, "sin nadie más que Claude Code no hay equipo");

        // El constructor de siempre es solo Claude Code: no hay a quién encargar.
        let err = work(&r, None, "arregla", &slug, None).unwrap_err().to_string();
        assert!(err.contains("además de Claude Code"), "{err}");
        // Una IA conectada que no está en ningún rol no cuenta.
        assert!(consult(&r, Some(&id), None, "revisa", None, None).unwrap_err().to_string().contains("no está en los roles"));

        set_config(&r, "constructor", &RoleConfig { candidates: vec![id.clone(), "claude-code".into()], strategy: "gratis".into() })
            .unwrap();
        let text = brief(&r, true).unwrap().unwrap();
        assert!(text.contains(&format!("Falso (ia: \"{id}\")")) && text.contains("puede cambiar archivos"), "{text}");
        assert!(!text.contains("claude-code"), "Claude Code no está en su propio equipo");

        let done = work(&r, None, "crea el archivo", &slug, Some("chat-1")).unwrap();
        assert!(done.starts_with("Falso trabajó en"), "{done}");
        assert!(project.join("hecho.txt").exists(), "escribe en la carpeta del proyecto");
        let answer = consult(&r, Some(&id), None, "¿qué ves?", Some(&slug), Some("chat-1")).unwrap();
        assert!(answer.starts_with("Respondió Falso"), "{answer}");

        // El chat ve en vivo quién trabajó, lo que fue diciendo y cuándo terminó.
        let seen = crate::activity::since(&r, "chat-1", 0).unwrap();
        let line = |e: &crate::activity::Entry| format!("{} {} {}", e.actor, e.kind, e.text);
        let lines: Vec<String> = seen.iter().map(line).collect();
        assert!(lines[0].starts_with("Falso inicio Encargo en"), "{lines:?}");
        assert!(lines.iter().any(|l| l.starts_with("Falso linea Listo:")), "la salida llega línea a línea: {lines:?}");
        assert!(lines.iter().filter(|l| l.starts_with("Falso fin")).count() == 2, "{lines:?}");
        assert!(lines.iter().any(|l| l.starts_with("Falso inicio Consulta: ¿qué ves?")), "{lines:?}");

        let usage = crate::ai::usage_summary(&r).unwrap();
        assert_eq!(usage["by_task"][0]["task"], "delegado");
        assert_eq!(usage["by_task"][0]["calls"], 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Un agente falso con nombre propio, que dice quién es y tarda un poco.
    fn named_agent(r: &Registry, dir: &std::path::Path, name: &str) -> String {
        let script = dir.join(format!("{name}.sh"));
        std::fs::write(&script, format!("#!/bin/sh\nsleep 2\necho \"Soy {name}\"\n")).unwrap();
        std::process::Command::new("chmod").arg("+x").arg(&script).status().unwrap();
        let command = format!("{} {{tarea}}", script.display());
        save(
            r,
            &Input {
                id: None,
                provider: "agente-personalizado".into(),
                name: name.into(),
                config: json!({ "command": command, "ask_command": command }),
                secret: None,
            },
        )
        .unwrap()
        .id
    }

    #[test]
    fn several_orders_run_at_once_and_spread_over_the_team() {
        let (r, dir) = registry("reparto");
        let uno = named_agent(&r, &dir, "Uno");
        let dos = named_agent(&r, &dir, "Dos");
        set_enabled(&r, true).unwrap();
        set_config(&r, "validador", &RoleConfig { candidates: vec![uno.clone(), dos.clone()], strategy: "primero".into() }).unwrap();

        let order = |task: &str| Order { task: task.into(), ..Default::default() };
        let start = std::time::Instant::now();
        let text = consult_many(&r, &[order("primera"), order("segunda"), order("")], Some("chat-r"))
            .unwrap_err()
            .to_string();
        assert!(text.contains("encargo 3 no dice"), "{text}");

        let text = consult_many(&r, &[order("primera"), order("segunda")], Some("chat-r")).unwrap();
        // Cada uno tarda 2 s: uno tras otro no bajarían de 4 s.
        assert!(start.elapsed() < std::time::Duration::from_millis(3900), "corren a la vez, no uno tras otro: {:?}", start.elapsed());
        assert!(text.starts_with("2 encargos en paralelo: 2 bien."), "{text}");
        let first = text.find("Soy Uno").expect("uno respondió");
        let second = text.find("Soy Dos").expect("el segundo encargo fue a otro del rol");
        assert!(first < second, "las respuestas van en el orden de los encargos: {text}");

        // Una IA que no está en el equipo falla sola; las demás siguen.
        let mixed = [order("ok"), Order { task: "x".into(), ai: Some("nadie".into()), ..Default::default() }];
        let text = consult_many(&r, &mixed, Some("chat-r")).unwrap();
        assert!(text.starts_with("2 encargos en paralelo: 1 bien, 1 no se pudieron."), "{text}");

        // Cada encargo deja su recuadro en vivo.
        let runs: std::collections::HashSet<String> =
            crate::activity::since(&r, "chat-r", 0).unwrap().into_iter().map(|e| e.run).collect();
        assert_eq!(runs.len(), 3);

        // Dos cambios en el mismo proyecto se pisarían.
        let same = [
            Order { task: "a".into(), slug: Some("demo".into()), ..Default::default() },
            Order { task: "b".into(), slug: Some("demo".into()), ..Default::default() },
        ];
        assert!(work_many(&r, &same, None).unwrap_err().to_string().contains("dos cambios para «demo»"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
