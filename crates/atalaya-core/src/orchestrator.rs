//! Orquestación: quién hace cada trabajo.
//!
//! Cada rol tiene una lista ordenada de candidatos (Claude Code, un agente de terminal
//! o un modelo por API) y una estrategia para elegir entre ellos. Si el elegido no está
//! disponible, falla o pasó su presupuesto, Atalaya sigue con el siguiente.
//!
//! - **constructor**: hace las mejoras en el código (Claude Code o un agente de terminal).
//! - **revisor**: revisa el cambio del constructor; siempre es otro modelo.
//! - **validador**: la ronda diaria; analiza los chequeos y deja decisiones.
//! - **asistente**: el chat de la app.

use crate::Registry;
use crate::connections::Connection;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::process::{Command, Stdio};

/// El Claude Code de la máquina, sin conexión guardada: lo que Atalaya usó siempre.
pub const CLAUDE_CODE: &str = "claude-code";

pub const ROLES: &[(&str, &str, &str)] = &[
    ("constructor", "Constructor", "Hace las mejoras en una copia del repo y deja un pull request."),
    ("revisor", "Revisor", "Revisa el cambio del constructor antes de que llegue a ti. Siempre es otro modelo."),
    ("validador", "Validador", "Hace la ronda diaria: revisa los chequeos y deja decisiones."),
    ("asistente", "Asistente", "Responde en el chat de la app."),
];

pub const STRATEGIES: &[(&str, &str)] = &[
    ("gratis", "Gratis primero"),
    ("primero", "El primero disponible"),
    ("barato", "El más barato"),
    ("rotar", "Rotar entre todos"),
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RoleConfig {
    /// Ids de conexión, o `claude-code`, en orden de preferencia.
    pub candidates: Vec<String>,
    /// `gratis`, `primero`, `barato` o `rotar`.
    pub strategy: String,
}

/// Quién puede hacer un trabajo.
#[derive(Debug, Clone)]
pub enum Candidate {
    ClaudeCode,
    Connection(Connection),
}

impl Candidate {
    pub fn id(&self) -> String {
        match self {
            Candidate::ClaudeCode => CLAUDE_CODE.into(),
            Candidate::Connection(c) => c.id.clone(),
        }
    }

    pub fn label(&self) -> String {
        match self {
            Candidate::ClaudeCode => "Claude Code".into(),
            Candidate::Connection(c) => match self.model() {
                Some(m) => format!("{} ({m})", c.name),
                None => c.name.clone(),
            },
        }
    }

    /// El modelo, cuando se sabe. Claude Code elige el suyo.
    pub fn model(&self) -> Option<String> {
        match self {
            Candidate::ClaudeCode => None,
            Candidate::Connection(c) => c.config_str("model"),
        }
    }

    /// Un agente de terminal (o Claude Code): trabaja sobre archivos, no solo responde.
    pub fn is_agent(&self) -> bool {
        match self {
            Candidate::ClaudeCode => true,
            Candidate::Connection(c) => c.kind == "agente",
        }
    }
}

fn key(role: &str) -> String {
    format!("ai_role:{role}")
}

fn check_role(role: &str) -> Result<()> {
    if !ROLES.iter().any(|(id, _, _)| *id == role) {
        bail!("rol desconocido '{role}'; usa {}", ROLES.iter().map(|r| r.0).collect::<Vec<_>>().join(", "));
    }
    Ok(())
}

/// Lo de siempre: Claude Code construye, valida y conversa; nadie revisa. Lo que se agregue
/// después se intenta gratis primero.
fn default_config(role: &str) -> RoleConfig {
    RoleConfig {
        candidates: if role == "revisor" { vec![] } else { vec![CLAUDE_CODE.into()] },
        strategy: "gratis".into(),
    }
}

/// Si el candidato puede trabajar sin costo: un modelo de este Mac, un servicio con uso
/// gratis o un agente que tiene versión gratis (o usa un modelo propio gratis).
pub fn is_free(candidate: &Candidate) -> bool {
    match candidate {
        Candidate::ClaudeCode => false,
        Candidate::Connection(c) if c.kind == "ia" => crate::connections::is_free(c),
        // Un agente que puede usar un modelo propio es gratis solo con uno gratis elegido.
        Candidate::Connection(c) if crate::agents::supports_own_model(&c.provider) => {
            crate::agents::own_model(c).is_some_and(|(base, model)| crate::connections::is_free_endpoint(&base, &model))
        }
        Candidate::Connection(c) => crate::clis::get(&c.provider).is_some_and(|cli| cli.free.is_some()),
    }
}

pub fn config(registry: &Registry, role: &str) -> Result<RoleConfig> {
    check_role(role)?;
    Ok(registry
        .setting(&key(role))?
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_else(|| default_config(role)))
}

/// Qué tipo de candidato sirve para cada rol.
fn fits(role: &str, c: &Connection) -> bool {
    let api = c.kind == "ia" && crate::ai::format_of(&c.provider).is_some();
    let agent = c.kind == "agente";
    // Los agentes de terminal sirven en todos los roles: para responder corren en solo lectura.
    match role {
        "constructor" => agent,
        _ => api || agent,
    }
}

pub fn set_config(registry: &Registry, role: &str, cfg: &RoleConfig) -> Result<()> {
    check_role(role)?;
    if !STRATEGIES.iter().any(|(id, _)| *id == cfg.strategy) {
        bail!("estrategia desconocida '{}'", cfg.strategy);
    }
    let mut seen = std::collections::HashSet::new();
    for id in &cfg.candidates {
        if !seen.insert(id) {
            bail!("'{id}' está dos veces en la lista");
        }
        if id == CLAUDE_CODE {
            continue;
        }
        let c = crate::connections::get(registry, id)?.with_context(|| format!("no existe la conexión '{id}'"))?;
        if !fits(role, &c) {
            bail!(
                "«{}» no sirve como {role}: {}",
                c.name,
                if role == "constructor" { "el constructor es Claude Code o un agente de terminal" } else { "este rol usa un modelo o un agente" }
            );
        }
    }
    registry.set_setting(&key(role), &serde_json::to_string(cfg)?)?;
    Ok(())
}

/// Convierte un id en candidato. Un agente guardado como «Claude Code» es el mismo Claude Code.
pub(crate) fn resolve(registry: &Registry, id: &str) -> Result<Option<Candidate>> {
    if id == CLAUDE_CODE {
        return Ok(Some(Candidate::ClaudeCode));
    }
    Ok(crate::connections::get(registry, id)?.map(|c| {
        if c.kind == "agente" && c.provider == "claude-code" { Candidate::ClaudeCode } else { Candidate::Connection(c) }
    }))
}

/// Por qué un candidato no puede trabajar ahora, o `None` si puede.
pub fn unavailable(registry: &Registry, candidate: &Candidate) -> Result<Option<String>> {
    if let Some(why) = crate::quota::unavailable(registry, &candidate.id())? {
        return Ok(Some(why));
    }
    Ok(match candidate {
        Candidate::ClaudeCode => {
            crate::worker::find_claude().is_none().then(|| "Claude Code no está instalado en este Mac".to_string())
        }
        Candidate::Connection(c) if c.kind == "agente" => {
            let program = crate::agents::program(c)?;
            if crate::agents::find_program(&program).is_none() {
                Some(format!("«{program}» no está instalado"))
            } else {
                crate::clis::unavailable(c)
            }
        }
        Candidate::Connection(c) => {
            let needs_key = crate::connections::provider(&c.provider).is_some_and(|p| p.secret.is_some());
            if needs_key && c.secret().is_none() {
                Some("falta su llave".into())
            } else if crate::ai::over_budget(registry, &c.id)? {
                Some("pasó su presupuesto del mes".into())
            } else {
                None
            }
        }
    })
}

/// Costo por millón de tokens (entrada + salida), si se sabe. Lo gratis vale 0.
fn unit_cost(registry: &Registry, candidate: &Candidate) -> Option<f64> {
    if is_free(candidate) {
        return Some(0.0);
    }
    match candidate {
        Candidate::Connection(c) if c.kind == "ia" => {
            let model = c.config_str("model")?;
            crate::ai::price(registry, c, &model).ok().flatten().map(|(i, o)| i + o)
        }
        _ => None,
    }
}

/// Los gratis adelante; cada grupo conserva el orden de la lista.
fn free_first(list: &mut [Candidate]) {
    list.sort_by_key(|c| !is_free(c));
}

/// Candidatos disponibles en el orden en que se intentarían, más los que se saltaron y por qué.
/// `exclude` quita candidatos (por ejemplo, el revisor no puede ser el constructor).
/// Solo mira: con la estrategia de rotar, el turno avanza recién al trabajar.
pub fn lineup(registry: &Registry, role: &str, exclude: &[Candidate]) -> Result<(Vec<Candidate>, Vec<String>)> {
    order(registry, role, exclude, false)
}

fn order(registry: &Registry, role: &str, exclude: &[Candidate], advance: bool) -> Result<(Vec<Candidate>, Vec<String>)> {
    let cfg = config(registry, role)?;
    let mut ready = Vec::new();
    let mut skipped = Vec::new();
    for id in &cfg.candidates {
        let Some(candidate) = resolve(registry, id)? else {
            skipped.push(format!("{id}: la conexión ya no existe"));
            continue;
        };
        if exclude.iter().any(|e| same_model(e, &candidate)) {
            skipped.push(format!("{}: es el mismo modelo que hizo el trabajo", candidate.label()));
            continue;
        }
        match unavailable(registry, &candidate)? {
            Some(why) => skipped.push(format!("{}: {why}", candidate.label())),
            None => ready.push(candidate),
        }
    }
    match cfg.strategy.as_str() {
        // Estable: los gratis adelante, cada grupo en el orden de la lista.
        "gratis" => free_first(&mut ready),
        "barato" => {
            // Estable: a igual precio (o sin precio) se respeta el orden de la lista.
            ready.sort_by(|a, b| {
                let (ca, cb) = (unit_cost(registry, a), unit_cost(registry, b));
                match (ca, cb) {
                    (Some(x), Some(y)) => x.total_cmp(&y),
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => std::cmp::Ordering::Equal,
                }
            });
        }
        "rotar" if !ready.is_empty() => {
            let turn_key = format!("ai_role_turn:{role}");
            let turn: usize = registry.setting(&turn_key)?.and_then(|t| t.parse().ok()).unwrap_or(0);
            let shift = turn % ready.len();
            ready.rotate_left(shift);
            if advance {
                registry.set_setting(&turn_key, &(turn + 1).to_string())?;
            }
        }
        _ => {}
    }
    Ok((ready, skipped))
}

/// Dos candidatos son el mismo modelo si son la misma conexión, los dos son Claude Code
/// o declaran el mismo modelo.
pub fn same_model(a: &Candidate, b: &Candidate) -> bool {
    if a.id() == b.id() {
        return true;
    }
    matches!((a.model(), b.model()), (Some(x), Some(y)) if x.eq_ignore_ascii_case(&y))
}

/// Intenta el trabajo con cada candidato del rol hasta que uno lo logre.
/// Devuelve el resultado y quién lo hizo; si nadie pudo, explica por qué con cada uno.
pub fn attempt<T>(
    registry: &Registry,
    role: &str,
    exclude: &[Candidate],
    mut work: impl FnMut(&Candidate) -> Result<T>,
) -> Result<(T, Candidate)> {
    let (ready, mut notes) = order(registry, role, exclude, true)?;
    if ready.is_empty() && notes.is_empty() {
        bail!("el rol {role} no tiene a nadie asignado; elígelo en Ajustes → IA");
    }
    for candidate in ready {
        match work(&candidate) {
            Ok(value) => return Ok((value, candidate)),
            Err(e) => notes.push(format!("{}: {e:#}", candidate.label())),
        }
    }
    bail!("nadie pudo hacer el trabajo de {role}. {}", notes.join(" · "))
}

/// Una pregunta de solo texto a Claude Code, sin herramientas ni sesión guardada. Queda
/// anotada en el consumo, salga bien o mal.
pub fn claude_text(registry: &Registry, system: &str, prompt: &str, task: &str, slug: Option<&str>) -> Result<String> {
    crate::ai::tracked_claude(registry, task, slug, |reply| {
        let claude = crate::worker::find_claude().context("no se encontró Claude Code en esta máquina (comando `claude`)")?;
        let out = Command::new(claude)
            .args(["-p", prompt, "--output-format", "json", "--tools", "", "--no-session-persistence"])
            .args(crate::ai::claude_cap_args(registry)?)
            .args(["--append-system-prompt", system])
            .env("PATH", crate::worker::developer_path())
            .current_dir(std::env::temp_dir())
            .stdin(Stdio::null())
            .output()?;
        let value: Value = serde_json::from_slice(&out.stdout).with_context(|| {
            let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
            if err.is_empty() { "Claude Code no respondió".to_string() } else { err }
        })?;
        let r = reply.insert(value);
        if r["is_error"].as_bool().unwrap_or(false) {
            let why = crate::ai::claude_cap_note(r).unwrap_or_else(|| r["result"].as_str().unwrap_or("sin detalle").to_string());
            bail!("Claude Code: {why}");
        }
        Ok(r["result"].as_str().unwrap_or_default().to_string())
    })
}

/// Una pregunta de texto a un candidato: Claude Code, un agente de terminal (en solo
/// lectura) o un modelo por API.
pub fn ask(registry: &Registry, candidate: &Candidate, system: &str, prompt: &str, task: &str, slug: Option<&str>) -> Result<String> {
    // Las instrucciones y skills del usuario: un agente de terminal las lee desde su
    // ruta; Claude Code sin herramientas y los modelos por API reciben su texto.
    let reader = match candidate {
        Candidate::Connection(c) if c.kind == "agente" => crate::skills::Reader::Files,
        _ => crate::skills::Reader::Inline,
    };
    let system = format!("{system}{}", crate::skills::for_ai(registry, slug, &candidate.id(), reader)?);
    let system = system.as_str();
    match candidate {
        Candidate::ClaudeCode => claude_text(registry, system, prompt, task, slug),
        Candidate::Connection(c) if c.kind == "agente" => crate::clis::ask(registry, c, system, prompt, task, slug),
        Candidate::Connection(c) => {
            let req = crate::ai::Request {
                system: Some(system.to_string()),
                messages: vec![crate::ai::Message::user(prompt.to_string())],
                max_tokens: 4096,
                task: task.to_string(),
                slug: slug.map(String::from),
                ..Default::default()
            };
            Ok(crate::ai::complete(registry, c, &req)?.text)
        }
    }
}

/// Saca el primer objeto JSON de una respuesta (los modelos a veces lo envuelven en texto o en ```).
pub fn extract_json(text: &str) -> Option<Value> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    (end > start).then(|| serde_json::from_str(&text[start..=end]).ok()).flatten()
}

/// Los roles, su configuración, quién está listo y quién puede elegirse. Para la app y el MCP.
pub fn summary(registry: &Registry) -> Result<Value> {
    let all = crate::connections::all(registry)?;
    let mut roles = Vec::new();
    for (id, label, description) in ROLES {
        let cfg = config(registry, id)?;
        let mut chosen = Vec::new();
        for cid in &cfg.candidates {
            let (name, status) = match resolve(registry, cid)? {
                Some(c) => (c.label(), unavailable(registry, &c)?),
                None => (cid.clone(), Some("la conexión ya no existe".into())),
            };
            chosen.push(json!({ "id": cid, "name": name, "ready": status.is_none(), "reason": status }));
        }
        // Claude Code sirve en todos los roles; como revisor, solo cuando construyó otro.
        let mut options = vec![json!({ "id": CLAUDE_CODE, "name": "Claude Code (el de este Mac)", "kind": "claude-code", "free": false })];
        for c in all.iter().filter(|c| fits(id, c) && !(c.kind == "agente" && c.provider == "claude-code")) {
            let candidate = Candidate::Connection(c.clone());
            let free = is_free(&candidate);
            options.push(json!({ "id": c.id, "name": candidate.label(), "kind": c.kind, "free": free }));
        }
        // Los gratis primero en «Agregar…».
        options.sort_by_key(|o| o["free"] != true);
        roles.push(json!({
            "id": id,
            "label": label,
            "description": description,
            "strategy": cfg.strategy,
            "candidates": chosen,
            "options": options,
        }));
    }
    Ok(json!({
        "roles": roles,
        "strategies": STRATEGIES.iter().map(|(id, label)| json!({ "id": id, "label": label })).collect::<Vec<_>>(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connections::{Input, save};

    fn temp_registry() -> (std::path::PathBuf, Registry) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!("atalaya-orq-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let registry = Registry::open(&dir.join("atalaya.db")).unwrap();
        (dir, registry)
    }

    fn local_ai(registry: &Registry, id: &str, model: &str) -> Connection {
        save(
            registry,
            &Input {
                id: None,
                provider: "ollama".into(),
                name: id.into(),
                config: json!({ "base_url": "http://127.0.0.1:9/v1", "model": model }),
                secret: None,
            },
        )
        .unwrap()
    }

    fn ids(list: &[Candidate]) -> Vec<String> {
        list.iter().map(Candidate::id).collect()
    }

    fn conn(provider: &str, kind: &str, config: Value) -> Candidate {
        Candidate::Connection(Connection {
            id: provider.into(),
            kind: kind.into(),
            provider: provider.into(),
            name: provider.into(),
            config,
            secret_account: None,
            created_at: String::new(),
        })
    }

    #[test]
    fn free_ones_go_first_keeping_the_list_order() {
        let paid = conn("openai-compatible", "ia", json!({ "base_url": "https://api.deepseek.com/v1", "model": "deepseek-chat" }));
        let router_paid = conn("openai-compatible", "ia", json!({ "base_url": "https://openrouter.ai/api/v1", "model": "x/y" }));
        let router_free = conn("openai-compatible", "ia", json!({ "base_url": "https://openrouter.ai/api/v1", "model": "x/y:free" }));
        let local = conn("ollama", "ia", json!({ "model": "qwen3.5:4b" }));
        let qwen_own = conn("qwen-code", "agente", json!({ "model_base_url": "http://127.0.0.1:11434/v1", "model": "qwen3.5:4b" }));
        let qwen_account = conn("qwen-code", "agente", json!({}));
        let antigravity = conn("antigravity", "agente", json!({}));
        let ollama_cloud = conn("ollama", "ia", json!({ "model": "glm-5.3-flash:cloud" }));
        let qwen_cloud = conn("qwen-code", "agente", json!({ "model_base_url": "http://127.0.0.1:11434/v1", "model": "glm-5.3-flash:cloud" }));
        assert!(!is_free(&ollama_cloud) && !is_free(&qwen_cloud), "los «:cloud» de Ollama se cobran");
        assert!(!is_free(&paid) && !is_free(&router_paid) && !is_free(&qwen_account) && !is_free(&Candidate::ClaudeCode));
        assert!(is_free(&router_free) && is_free(&local) && is_free(&qwen_own) && is_free(&antigravity));

        let mut list = vec![Candidate::ClaudeCode, paid, local, router_free];
        free_first(&mut list);
        let order: Vec<String> = list.iter().map(|c| c.label()).collect();
        assert_eq!(order, ["ollama (qwen3.5:4b)", "openai-compatible (x/y:free)", "Claude Code", "openai-compatible (deepseek-chat)"]);
    }

    #[test]
    fn defaults_keep_claude_code_and_no_reviewer() {
        let (_d, r) = temp_registry();
        assert_eq!(config(&r, "constructor").unwrap().candidates, vec![CLAUDE_CODE.to_string()]);
        assert!(config(&r, "revisor").unwrap().candidates.is_empty());
        assert!(config(&r, "nadie").is_err());
    }

    #[test]
    fn roles_only_take_candidates_that_fit() {
        let (_d, r) = temp_registry();
        local_ai(&r, "a", "llama3");
        let bad = RoleConfig { candidates: vec!["a".into()], strategy: "primero".into() };
        assert!(set_config(&r, "constructor", &bad).is_err(), "un modelo por API no construye");
        set_config(&r, "revisor", &bad).unwrap();
        let twice = RoleConfig { candidates: vec!["a".into(), "a".into()], strategy: "primero".into() };
        assert!(set_config(&r, "revisor", &twice).is_err());
        let unknown = RoleConfig { candidates: vec!["a".into()], strategy: "azar".into() };
        assert!(set_config(&r, "revisor", &unknown).is_err());
    }

    #[test]
    fn strategies_order_and_rotate() {
        let (_d, r) = temp_registry();
        local_ai(&r, "a", "m1");
        local_ai(&r, "b", "m2");
        let mut cfg = RoleConfig { candidates: vec!["a".into(), "b".into()], strategy: "primero".into() };
        set_config(&r, "validador", &cfg).unwrap();
        assert_eq!(ids(&lineup(&r, "validador", &[]).unwrap().0), ["a", "b"]);

        cfg.strategy = "rotar".into();
        set_config(&r, "validador", &cfg).unwrap();
        let first = |r: &Registry| attempt(r, "validador", &[], |c| Ok(c.id())).unwrap().0;
        assert_eq!(first(&r), "a");
        assert_eq!(first(&r), "b");
        // Mirar la lista no gasta un turno.
        assert_eq!(ids(&lineup(&r, "validador", &[]).unwrap().0), ["a", "b"]);
        assert_eq!(first(&r), "a");
    }

    #[test]
    fn falls_back_on_failure_and_budget_and_reviewer_differs() {
        let (_d, r) = temp_registry();
        let a = local_ai(&r, "a", "m1");
        local_ai(&r, "b", "m2");
        let c = local_ai(&r, "c", "m1");
        set_config(&r, "revisor", &RoleConfig { candidates: vec!["a".into(), "b".into(), "c".into()], strategy: "primero".into() })
            .unwrap();

        // El primero falla: sigue con el segundo.
        let (value, who) = attempt(&r, "revisor", &[], |cand| {
            if cand.id() == "a" { bail!("se cayó") } else { Ok(cand.id()) }
        })
        .unwrap();
        assert_eq!((value.as_str(), who.id().as_str()), ("b", "b"));

        // Si todos fallan, el error cuenta qué pasó con cada uno.
        let err = attempt(&r, "revisor", &[], |_| -> Result<()> { bail!("sin red") }).unwrap_err().to_string();
        assert!(err.contains("sin red") && err.contains("a (m1)"), "{err}");

        // El revisor no puede ser el mismo modelo que construyó: ni la misma conexión ni otra con el mismo modelo.
        let (ready, skipped) = lineup(&r, "revisor", &[Candidate::Connection(a.clone())]).unwrap();
        assert_eq!(ids(&ready), ["b"]);
        assert_eq!(skipped.len(), 2);
        let _ = c;

        // Pasado el presupuesto, se salta. Un modelo local no gasta, así que se fija un precio
        // a mano en una conexión remota imitada con una llamada anotada.
        crate::ai::set_budget(&r, "b", Some(0.01)).unwrap();
        r.conn()
            .execute(
                "INSERT INTO ai_usage (connection_id, provider, model, task, input_tokens, output_tokens, cost_usd, ok)
                 VALUES ('b', 'ollama', 'm2', 'prueba', 1, 1, 5.0, 1)",
                [],
            )
            .unwrap();
        let (ready, skipped) = lineup(&r, "revisor", &[Candidate::Connection(a)]).unwrap();
        assert!(ready.is_empty());
        assert!(skipped.iter().any(|s| s.contains("presupuesto")), "{skipped:?}");
    }

    #[test]
    fn skips_one_without_quota_until_a_good_call() {
        let (_d, r) = temp_registry();
        let a = local_ai(&r, "a", "m1");
        local_ai(&r, "b", "m2");
        set_config(&r, "revisor", &RoleConfig { candidates: vec!["a".into(), "b".into()], strategy: "primero".into() })
            .unwrap();
        // Falla por cuota: queda anotado el motivo y se salta hasta que se renueve.
        crate::ai::record(&r, &a, "m1", "agente", None, (None, None), None, Some("terminó con error: You've hit your usage limit"))
            .unwrap();
        let (ready, skipped) = lineup(&r, "revisor", &[]).unwrap();
        assert_eq!(ids(&ready), ["b"]);
        assert!(skipped.iter().any(|s| s.contains("sin cuota hasta")), "{skipped:?}");
        // Una llamada buena (por ejemplo, «probar» en Conexiones) la devuelve a la lista.
        crate::ai::record(&r, &a, "m1", "prueba", None, (None, None), None, None).unwrap();
        assert_eq!(ids(&lineup(&r, "revisor", &[]).unwrap().0), ["a", "b"]);
    }

    #[test]
    fn finds_json_inside_text() {
        let v = extract_json("Aquí va:\n```json\n{\"veredicto\": \"aprobado\", \"comentarios\": \"bien\"}\n```").unwrap();
        assert_eq!(v["veredicto"], "aprobado");
        assert!(extract_json("sin json").is_none());
    }
}
