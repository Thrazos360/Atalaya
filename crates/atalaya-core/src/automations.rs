//! Lo que Atalaya hace solo, sin que se lo pidas en el momento:
//! - **Tareas programadas**: un encargo de solo lectura que corre cada día, cada semana o cada
//!   tantas horas («resume la semana de Faro», «/revisar-seguridad faro»), con la IA que elijas.
//! - **Reacciones**: cuando una revisión pasa a falla (un despliegue, el sitio, GitHub…), una
//!   IA diagnostica la causa apenas la Vigía lo detecta, y si quieres deja una mejora propuesta.
//!
//! Las dos solo leen: no cambian código ni datos. Lo que encuentran llega como aviso (y por
//! Telegram si está configurado) y queda en el historial. Las corre la Vigía (`atalaya watch`).

use crate::Registry;
use crate::registry::Check;
use anyhow::{Context, Result, bail};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub fn init(registry: &Registry) -> Result<()> {
    registry.conn().execute_batch(
        "CREATE TABLE IF NOT EXISTS schedules (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            slug TEXT,
            prompt TEXT NOT NULL,
            ai TEXT,
            every TEXT NOT NULL DEFAULT 'dia',
            at TEXT NOT NULL DEFAULT '09:00',
            weekday INTEGER,
            hours INTEGER,
            enabled INTEGER NOT NULL DEFAULT 1,
            last_run TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE TABLE IF NOT EXISTS reactions (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            kinds TEXT NOT NULL DEFAULT '[]',
            slug TEXT,
            ai TEXT,
            improvement INTEGER NOT NULL DEFAULT 0,
            enabled INTEGER NOT NULL DEFAULT 1,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE TABLE IF NOT EXISTS automation_runs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            source TEXT NOT NULL,
            source_id INTEGER NOT NULL,
            name TEXT NOT NULL,
            slug TEXT,
            kind TEXT,
            ai TEXT NOT NULL DEFAULT '',
            ok INTEGER NOT NULL,
            text TEXT NOT NULL,
            improvement_id INTEGER,
            at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS automation_runs_at ON automation_runs (at);",
    )?;
    Ok(())
}

/// Cada cuánto corre una tarea.
pub const EVERY: [&str; 3] = ["dia", "semana", "horas"];

/// Días de la semana, como los cuenta SQLite (`%w`): 0 es domingo.
pub const WEEKDAYS: [&str; 7] = ["domingo", "lunes", "martes", "miércoles", "jueves", "viernes", "sábado"];

/// Una reacción no se repite para el mismo proyecto y revisión antes de esto.
const QUIET_HOURS: i64 = 6;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Schedule {
    #[serde(default)]
    pub id: i64,
    pub name: String,
    pub slug: Option<String>,
    /// El encargo. Puede ser un comando guardado: `/resumen-semana faro`.
    pub prompt: String,
    /// La IA (conexión o `claude-code`); sin ella, la primera disponible del validador.
    pub ai: Option<String>,
    /// `dia`, `semana` o `horas`.
    pub every: String,
    /// `HH:MM`, hora local (para `dia` y `semana`).
    pub at: String,
    /// 0 domingo … 6 sábado (para `semana`).
    pub weekday: Option<i64>,
    /// Cada cuántas horas (para `horas`).
    pub hours: Option<i64>,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub last_run: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Reaction {
    #[serde(default)]
    pub id: i64,
    pub name: String,
    /// Las revisiones que la disparan (`despliegue`, `sitio`, `github`…); vacía, todas.
    #[serde(default)]
    pub kinds: Vec<String>,
    pub slug: Option<String>,
    pub ai: Option<String>,
    /// Además del aviso, deja el diagnóstico como mejora propuesta.
    #[serde(default)]
    pub improvement: bool,
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Serialize)]
pub struct Run {
    pub id: i64,
    /// `tarea` o `reaccion`.
    pub source: String,
    pub source_id: i64,
    pub name: String,
    pub slug: Option<String>,
    pub kind: Option<String>,
    pub ai: String,
    pub ok: bool,
    pub text: String,
    pub improvement_id: Option<i64>,
    pub at: String,
}

/* ------------------------------------------------------------------ Tareas */

fn schedule_row(r: &rusqlite::Row) -> rusqlite::Result<Schedule> {
    Ok(Schedule {
        id: r.get(0)?,
        name: r.get(1)?,
        slug: r.get(2)?,
        prompt: r.get(3)?,
        ai: r.get(4)?,
        every: r.get(5)?,
        at: r.get(6)?,
        weekday: r.get(7)?,
        hours: r.get(8)?,
        enabled: r.get(9)?,
        last_run: r.get(10)?,
    })
}

const SCHEDULE_COLUMNS: &str = "id, name, slug, prompt, ai, every, at, weekday, hours, enabled, last_run";

pub fn schedules(registry: &Registry) -> Result<Vec<Schedule>> {
    let mut stmt = registry.conn().prepare(&format!("SELECT {SCHEDULE_COLUMNS} FROM schedules ORDER BY id"))?;
    Ok(stmt.query_map([], schedule_row)?.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn schedule(registry: &Registry, id: i64) -> Result<Option<Schedule>> {
    Ok(registry
        .conn()
        .query_row(&format!("SELECT {SCHEDULE_COLUMNS} FROM schedules WHERE id = ?1"), [id], schedule_row)
        .optional()?)
}

fn clean_slug(registry: &Registry, slug: Option<&str>) -> Result<Option<String>> {
    match slug.map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok(None),
        Some(s) => Ok(Some(registry.get(s)?.with_context(|| format!("no hay un proyecto «{s}»"))?.slug)),
    }
}

fn clean_ai(registry: &Registry, ai: Option<&str>) -> Result<Option<String>> {
    match ai.map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok(None),
        Some(id) => {
            if crate::orchestrator::resolve(registry, id)?.is_none() {
                bail!("no hay una IA «{id}»");
            }
            Ok(Some(id.to_string()))
        }
    }
}

fn clean_time(at: &str) -> Result<String> {
    let at = at.trim();
    let (h, m) = at.split_once(':').with_context(|| format!("la hora «{at}» va como HH:MM"))?;
    let (h, m): (u32, u32) = (h.parse().context("la hora va como HH:MM")?, m.parse().context("la hora va como HH:MM")?);
    if h > 23 || m > 59 {
        bail!("la hora «{at}» no existe");
    }
    Ok(format!("{h:02}:{m:02}"))
}

/// Guarda una tarea (nueva si `id` es 0). Una tarea nueva no corre por un horario que ya
/// pasó hoy: espera al próximo.
pub fn save_schedule(registry: &Registry, s: &Schedule) -> Result<Schedule> {
    let name = s.name.trim();
    if name.is_empty() {
        bail!("ponle un nombre a la tarea");
    }
    let prompt = s.prompt.trim();
    if prompt.is_empty() {
        bail!("escribe qué tiene que hacer la tarea");
    }
    if !EVERY.contains(&s.every.as_str()) {
        bail!("cada cuánto: dia, semana u horas");
    }
    let at = clean_time(if s.at.trim().is_empty() { "09:00" } else { &s.at })?;
    let weekday = match s.every.as_str() {
        "semana" => Some(s.weekday.filter(|d| (0..7).contains(d)).context("elige el día de la semana")?),
        _ => None,
    };
    let hours = match s.every.as_str() {
        "horas" => Some(s.hours.filter(|h| (1..=168).contains(h)).context("cada cuántas horas: de 1 a 168")?),
        _ => None,
    };
    let slug = clean_slug(registry, s.slug.as_deref())?;
    let ai = clean_ai(registry, s.ai.as_deref())?;
    let now = local_now(registry)?;
    if s.id == 0 {
        registry.conn().execute(
            "INSERT INTO schedules (name, slug, prompt, ai, every, at, weekday, hours, enabled, last_run)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![name, slug, prompt, ai, s.every, at, weekday, hours, s.enabled, now],
        )?;
        let id = registry.conn().last_insert_rowid();
        return Ok(schedule(registry, id)?.expect("recién creada"));
    }
    let changed = registry.conn().execute(
        "UPDATE schedules SET name = ?2, slug = ?3, prompt = ?4, ai = ?5, every = ?6, at = ?7, weekday = ?8, hours = ?9,
             enabled = ?10 WHERE id = ?1",
        params![s.id, name, slug, prompt, ai, s.every, at, weekday, hours, s.enabled],
    )?;
    if changed == 0 {
        bail!("no hay una tarea #{}", s.id);
    }
    Ok(schedule(registry, s.id)?.expect("existe"))
}

pub fn set_schedule_enabled(registry: &Registry, id: i64, on: bool) -> Result<()> {
    if registry.conn().execute("UPDATE schedules SET enabled = ?2 WHERE id = ?1", params![id, on])? == 0 {
        bail!("no hay una tarea #{id}");
    }
    Ok(())
}

/// Quita una tarea. Su historial queda.
pub fn remove_schedule(registry: &Registry, id: i64) -> Result<()> {
    if registry.conn().execute("DELETE FROM schedules WHERE id = ?1", [id])? == 0 {
        bail!("no hay una tarea #{id}");
    }
    Ok(())
}

/// Fecha y hora local, como texto (`AAAA-MM-DD HH:MM:SS`), y el día de la semana (0 domingo).
fn local_clock(registry: &Registry) -> Result<(String, i64)> {
    Ok(registry.conn().query_row(
        "SELECT datetime('now', 'localtime'), CAST(strftime('%w', 'now', 'localtime') AS INTEGER)",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?)
}

fn local_now(registry: &Registry) -> Result<String> {
    Ok(local_clock(registry)?.0)
}

/// Si le toca correr a una tarea en este momento (`now` en hora local, `weekday` 0 domingo).
pub fn is_due(s: &Schedule, now: &str, weekday: i64) -> bool {
    if !s.enabled {
        return false;
    }
    let last = s.last_run.as_deref().unwrap_or("");
    match s.every.as_str() {
        "horas" => {
            let Some(hours) = s.hours else { return false };
            last.is_empty() || add_hours(last, hours).is_some_and(|next| next.as_str() <= now)
        }
        every => {
            if every == "semana" && s.weekday != Some(weekday) {
                return false;
            }
            let slot = format!("{} {}:00", &now[..10.min(now.len())], s.at);
            now >= slot.as_str() && last < slot.as_str()
        }
    }
}

/// `AAAA-MM-DD HH:MM:SS` más unas horas, sin depender de otra biblioteca de fechas.
fn add_hours(at: &str, hours: i64) -> Option<String> {
    let conn = rusqlite::Connection::open_in_memory().ok()?;
    conn.query_row("SELECT datetime(?1, ?2)", params![at, format!("+{hours} hours")], |r| r.get(0)).ok()
}

/// Corre las tareas a las que les toca. Devuelve cuántas corrió.
pub fn round(registry: &Registry) -> Result<usize> {
    let (now, weekday) = local_clock(registry)?;
    let due: Vec<Schedule> = schedules(registry)?.into_iter().filter(|s| is_due(s, &now, weekday)).collect();
    for s in &due {
        // Se marca antes de correr: si tarda o falla, no se repite en la próxima vuelta.
        registry.conn().execute("UPDATE schedules SET last_run = ?2 WHERE id = ?1", params![s.id, now])?;
        if let Err(e) = run_schedule(registry, s) {
            eprintln!("la tarea «{}» falló: {e:#}", s.name);
        }
    }
    Ok(due.len())
}

/// Corre una tarea ahora, le toque o no.
pub fn run_now(registry: &Registry, id: i64) -> Result<Run> {
    let s = schedule(registry, id)?.with_context(|| format!("no hay una tarea #{id}"))?;
    run_schedule(registry, &s)
}

const SCHEDULE_SYSTEM: &str = "Atalaya te encarga una tarea programada del usuario. Responde en español, breve y \
concreto: lo importante primero. No cambies archivos ni datos. Si no pudiste comprobar algo, dilo.";

fn run_schedule(registry: &Registry, s: &Schedule) -> Result<Run> {
    let prompt = crate::commands::expand(registry, &s.prompt)?.unwrap_or_else(|| s.prompt.clone());
    let (ai, ok, text) = ask(registry, s.ai.as_deref(), SCHEDULE_SYSTEM, &prompt, s.slug.as_deref(), "tarea")?;
    let run = record(registry, "tarea", s.id, &s.name, s.slug.as_deref(), None, &ai, ok, &text, None)?;
    let title = if ok { format!("Tarea «{}»", s.name) } else { format!("La tarea «{}» no pudo terminar", s.name) };
    tell(registry, &title, &text);
    Ok(run)
}

/* --------------------------------------------------------------- Reacciones */

fn reaction_row(r: &rusqlite::Row) -> rusqlite::Result<Reaction> {
    let kinds: String = r.get(2)?;
    Ok(Reaction {
        id: r.get(0)?,
        name: r.get(1)?,
        kinds: serde_json::from_str(&kinds).unwrap_or_default(),
        slug: r.get(3)?,
        ai: r.get(4)?,
        improvement: r.get(5)?,
        enabled: r.get(6)?,
    })
}

const REACTION_COLUMNS: &str = "id, name, kinds, slug, ai, improvement, enabled";

pub fn reactions(registry: &Registry) -> Result<Vec<Reaction>> {
    let mut stmt = registry.conn().prepare(&format!("SELECT {REACTION_COLUMNS} FROM reactions ORDER BY id"))?;
    Ok(stmt.query_map([], reaction_row)?.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn save_reaction(registry: &Registry, x: &Reaction) -> Result<Reaction> {
    let name = x.name.trim();
    if name.is_empty() {
        bail!("ponle un nombre a la reacción");
    }
    let known: Vec<&str> = crate::checks::KINDS.iter().map(|(k, _)| *k).collect();
    if let Some(bad) = x.kinds.iter().find(|k| !known.contains(&k.as_str())) {
        bail!("«{bad}» no es una revisión; son: {}", known.join(", "));
    }
    let kinds = serde_json::to_string(&x.kinds)?;
    let slug = clean_slug(registry, x.slug.as_deref())?;
    let ai = clean_ai(registry, x.ai.as_deref())?;
    let id = if x.id == 0 {
        registry.conn().execute(
            "INSERT INTO reactions (name, kinds, slug, ai, improvement, enabled) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![name, kinds, slug, ai, x.improvement, x.enabled],
        )?;
        registry.conn().last_insert_rowid()
    } else {
        let changed = registry.conn().execute(
            "UPDATE reactions SET name = ?2, kinds = ?3, slug = ?4, ai = ?5, improvement = ?6, enabled = ?7 WHERE id = ?1",
            params![x.id, name, kinds, slug, ai, x.improvement, x.enabled],
        )?;
        if changed == 0 {
            bail!("no hay una reacción #{}", x.id);
        }
        x.id
    };
    Ok(registry
        .conn()
        .query_row(&format!("SELECT {REACTION_COLUMNS} FROM reactions WHERE id = ?1"), [id], reaction_row)?)
}

pub fn set_reaction_enabled(registry: &Registry, id: i64, on: bool) -> Result<()> {
    if registry.conn().execute("UPDATE reactions SET enabled = ?2 WHERE id = ?1", params![id, on])? == 0 {
        bail!("no hay una reacción #{id}");
    }
    Ok(())
}

pub fn remove_reaction(registry: &Registry, id: i64) -> Result<()> {
    if registry.conn().execute("DELETE FROM reactions WHERE id = ?1", [id])? == 0 {
        bail!("no hay una reacción #{id}");
    }
    Ok(())
}

/// Las fallas nuevas de esta vuelta de revisiones, con la reacción que les toca.
pub fn triggered(registry: &Registry, results: &[(Check, Option<String>)]) -> Result<Vec<(Reaction, Check)>> {
    let all: Vec<Reaction> = reactions(registry)?.into_iter().filter(|x| x.enabled).collect();
    let mut out = Vec::new();
    for (check, previous) in results {
        if check.status != crate::checks::FAIL || previous.as_deref() == Some(crate::checks::FAIL) {
            continue;
        }
        for x in &all {
            let kind_ok = x.kinds.is_empty() || x.kinds.contains(&check.kind);
            let slug_ok = x.slug.as_deref().is_none_or(|s| s == check.slug);
            if !kind_ok || !slug_ok {
                continue;
            }
            // Una revisión que sube y baja no dispara una reacción tras otra.
            let recent: bool = registry.conn().query_row(
                "SELECT EXISTS (SELECT 1 FROM automation_runs WHERE source = 'reaccion' AND source_id = ?1 AND slug = ?2
                   AND kind = ?3 AND at >= datetime('now', ?4))",
                params![x.id, check.slug, check.kind, format!("-{QUIET_HOURS} hours")],
                |r| r.get(0),
            )?;
            if !recent {
                out.push((x.clone(), check.clone()));
            }
        }
    }
    Ok(out)
}

/// Reacciona a las fallas nuevas. En la Vigía corre en otro hilo para no atrasar las
/// revisiones (`background`); en las pruebas, en el mismo.
pub fn react(registry: &Registry, results: &[(Check, Option<String>)], background: bool) -> Result<usize> {
    let jobs = triggered(registry, results)?;
    let n = jobs.len();
    if n == 0 {
        return Ok(0);
    }
    // Se anota antes de diagnosticar: la próxima vuelta ya no la repite aunque siga corriendo.
    let mut pending = Vec::new();
    for (x, check) in jobs {
        let id = record(registry, "reaccion", x.id, &x.name, Some(&check.slug), Some(&check.kind), "", false, "Diagnosticando…", None)?.id;
        pending.push((id, x, check));
    }
    if background {
        let path = registry.path();
        std::thread::spawn(move || {
            let Ok(registry) = Registry::open(&path) else { return };
            for (id, x, check) in pending {
                if let Err(e) = diagnose(&registry, id, &x, &check) {
                    eprintln!("la reacción «{}» falló: {e:#}", x.name);
                }
            }
        });
    } else {
        for (id, x, check) in pending {
            diagnose(registry, id, &x, &check)?;
        }
    }
    Ok(n)
}

const REACTION_SYSTEM: &str = "Atalaya detectó una falla en un proyecto del usuario y te pide un diagnóstico. \
Responde en español, en pocas líneas: la causa más probable, qué revisar para confirmarla y cómo arreglarlo. No \
cambies archivos ni datos. No inventes: si con lo que tienes no alcanza, dilo y di qué mirarías.";

fn diagnose(registry: &Registry, run_id: i64, x: &Reaction, check: &Check) -> Result<()> {
    let name = registry.get(&check.slug)?.map(|p| p.name).unwrap_or_else(|| check.slug.clone());
    let details = clip(&check.details.to_string(), 4_000);
    let prompt = format!(
        "Falló la revisión «{}» del proyecto «{name}»: {}\n\nDetalle de la revisión (JSON):\n{details}",
        check.kind, check.summary
    );
    let (ai, ok, text) = ask(registry, x.ai.as_deref(), REACTION_SYSTEM, &prompt, Some(&check.slug), "reaccion")?;
    let mut improvement_id = None;
    if ok && x.improvement {
        let title = format!("Arreglar {}: {}", check.kind, clip(check.summary.lines().next().unwrap_or_default(), 90));
        improvement_id = Some(registry.add_improvement(&check.slug, &title, &format!("Diagnóstico de {ai}:\n\n{text}"), "reaccion")?);
    }
    registry.conn().execute(
        "UPDATE automation_runs SET ai = ?2, ok = ?3, text = ?4, improvement_id = ?5 WHERE id = ?1",
        params![run_id, ai, ok, text, improvement_id],
    )?;
    let title = format!("{name}: falla en {} · diagnóstico de {ai}", check.kind);
    let extra = improvement_id.map(|id| format!("\n\nQuedó como mejora propuesta #{id}.")).unwrap_or_default();
    tell(registry, &title, &format!("{text}{extra}"));
    Ok(())
}

/* ------------------------------------------------------------------- Común */

/// Hace la pregunta con la IA pedida o, sin ella, con la primera disponible del validador.
/// Devuelve quién respondió, si pudo y el texto (o por qué no pudo).
fn ask(registry: &Registry, ai: Option<&str>, system: &str, prompt: &str, slug: Option<&str>, task: &str) -> Result<(String, bool, String)> {
    let id = match ai {
        Some(id) => id.to_string(),
        None => {
            let (ready, skipped) = crate::orchestrator::lineup(registry, "validador", &[])?;
            match ready.first() {
                Some(c) => c.id(),
                None => {
                    let why = if skipped.is_empty() { String::new() } else { format!(" ({})", skipped.join(" · ")) };
                    return Ok(("nadie".into(), false, format!("el validador no tiene una IA disponible{why}")));
                }
            }
        }
    };
    let answer = crate::compare::ask_as(registry, &id, system, prompt, slug, task);
    Ok((answer.name, answer.ok, answer.text))
}

#[allow(clippy::too_many_arguments)]
fn record(
    registry: &Registry,
    source: &str,
    source_id: i64,
    name: &str,
    slug: Option<&str>,
    kind: Option<&str>,
    ai: &str,
    ok: bool,
    text: &str,
    improvement_id: Option<i64>,
) -> Result<Run> {
    registry.conn().execute(
        "INSERT INTO automation_runs (source, source_id, name, slug, kind, ai, ok, text, improvement_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![source, source_id, name, slug, kind, ai, ok, text, improvement_id],
    )?;
    let id = registry.conn().last_insert_rowid();
    Ok(runs(registry, 1)?.into_iter().find(|r| r.id == id).expect("recién anotada"))
}

/// Lo último que corrieron las tareas y las reacciones, lo más nuevo primero.
pub fn runs(registry: &Registry, limit: usize) -> Result<Vec<Run>> {
    let mut stmt = registry.conn().prepare(
        "SELECT id, source, source_id, name, slug, kind, ai, ok, text, improvement_id, at FROM automation_runs
         ORDER BY id DESC LIMIT ?1",
    )?;
    let rows = stmt.query_map([limit as i64], |r| {
        Ok(Run {
            id: r.get(0)?,
            source: r.get(1)?,
            source_id: r.get(2)?,
            name: r.get(3)?,
            slug: r.get(4)?,
            kind: r.get(5)?,
            ai: r.get(6)?,
            ok: r.get(7)?,
            text: r.get(8)?,
            improvement_id: r.get(9)?,
            at: r.get(10)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Para la app: tareas, reacciones, historial y las revisiones que pueden disparar una reacción.
pub fn summary(registry: &Registry) -> Result<Value> {
    Ok(json!({
        "schedules": schedules(registry)?,
        "reactions": reactions(registry)?,
        "runs": runs(registry, 40)?,
        "kinds": crate::checks::KINDS.iter().map(|(k, _)| *k).collect::<Vec<_>>(),
    }))
}

/// Avisa en el Mac y, si está configurado, por Telegram.
fn tell(registry: &Registry, title: &str, text: &str) {
    crate::checks::notify(registry, "tareas", title, &clip(text, 300));
    crate::checks::telegram(registry, &format!("{title}\n\n{}", clip(text, 3_000)));
}

fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max { text.to_string() } else { format!("{}…", text.chars().take(max).collect::<String>()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connections::{Input, save};

    fn agent(r: &Registry, dir: &std::path::Path, name: &str) -> String {
        let script = dir.join(format!("{name}.sh"));
        std::fs::write(&script, format!("#!/bin/sh\necho \"{name}: revisé.\"\nprintf '%s' \"$1\"\n")).unwrap();
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

    fn setup(name: &str) -> (Registry, std::path::PathBuf, String) {
        let dir = std::env::temp_dir().join(format!("atalaya-auto-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let r = Registry::open(&dir.join("a.db")).unwrap();
        crate::testutil::project(&r, "faro");
        let ai = agent(&r, &dir, "Vigilante");
        (r, dir, ai)
    }

    fn task(every: &str, at: &str) -> Schedule {
        Schedule { name: "t".into(), prompt: "x".into(), every: every.into(), at: at.into(), enabled: true, ..Default::default() }
    }

    #[test]
    fn when_a_task_is_due() {
        let mut s = task("dia", "09:00");
        s.last_run = Some("2026-11-23 15:00:00".into());
        assert!(!is_due(&s, "2026-11-24 08:59:00", 2), "todavía no es la hora");
        assert!(is_due(&s, "2026-11-24 09:00:30", 2));
        s.last_run = Some("2026-11-24 09:00:30".into());
        assert!(!is_due(&s, "2026-11-24 18:00:00", 2), "ya corrió hoy");
        // Creada hoy a las 15:00, no corre por las 09:00 que ya pasaron.
        s.last_run = Some("2026-11-24 15:00:00".into());
        assert!(!is_due(&s, "2026-11-24 15:01:00", 2));

        let mut w = task("semana", "08:00");
        w.weekday = Some(1);
        w.last_run = Some("2026-11-20 10:00:00".into());
        assert!(!is_due(&w, "2026-11-24 09:00:00", 2), "martes no es lunes");
        assert!(is_due(&w, "2026-11-23 08:30:00", 1));

        let mut h = task("horas", "");
        h.hours = Some(6);
        h.last_run = Some("2026-11-24 03:00:00".into());
        assert!(!is_due(&h, "2026-11-24 08:59:59", 2));
        assert!(is_due(&h, "2026-11-24 09:00:00", 2));
        h.enabled = false;
        assert!(!is_due(&h, "2026-11-24 23:00:00", 2), "pausada no corre");
    }

    #[test]
    fn a_task_runs_with_its_ai_and_a_saved_command() {
        let (r, dir, ai) = setup("tarea");
        assert!(save_schedule(&r, &Schedule { name: "x".into(), prompt: "y".into(), every: "semana".into(), ..Default::default() }).is_err());
        crate::commands::add_examples(&r).unwrap();
        let s = save_schedule(
            &r,
            &Schedule {
                name: "Semana de Faro".into(),
                slug: Some("faro".into()),
                prompt: "/resumen-semana Faro".into(),
                ai: Some(ai.clone()),
                every: "semana".into(),
                at: "9:5".into(),
                weekday: Some(1),
                enabled: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(s.at, "09:05");
        assert!(s.last_run.is_some(), "no corre por un horario que ya pasó");
        let run = run_now(&r, s.id).unwrap();
        assert!(run.ok, "{}", run.text);
        assert_eq!(run.ai, "Vigilante");
        assert!(run.text.contains("Vigilante: revisé."), "{}", run.text);
        assert!(run.text.contains("Resume lo que pasó esta semana en Faro"), "el comando guardado se arma: {}", run.text);
        assert_eq!(runs(&r, 10).unwrap().len(), 1);
        set_schedule_enabled(&r, s.id, false).unwrap();
        assert!(!schedule(&r, s.id).unwrap().unwrap().enabled);
        remove_schedule(&r, s.id).unwrap();
        assert_eq!(runs(&r, 10).unwrap().len(), 1, "el historial queda");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn check(kind: &str, status: &str) -> Check {
        Check {
            slug: "faro".into(),
            kind: kind.into(),
            status: status.into(),
            summary: "El último despliegue falló: Build error".into(),
            details: json!({ "error": "Module not found" }),
            checked_at: String::new(),
            acked: false,
        }
    }

    #[test]
    fn a_new_failure_gets_a_diagnosis_once() {
        let (r, dir, ai) = setup("reaccion");
        assert!(save_reaction(&r, &Reaction { name: "x".into(), kinds: vec!["nada".into()], ..Default::default() }).is_err());
        let x = save_reaction(
            &r,
            &Reaction { name: "Despliegues".into(), kinds: vec!["despliegue".into()], ai: Some(ai), improvement: true, enabled: true, ..Default::default() },
        )
        .unwrap();

        // Lo que no es una falla nueva, o es otra revisión, no dispara nada.
        let quiet = [
            (check("despliegue", "ok"), Some("falla".to_string())),
            (check("despliegue", "falla"), Some("falla".to_string())),
            (check("sitio", "falla"), Some("ok".to_string())),
        ];
        assert_eq!(react(&r, &quiet, false).unwrap(), 0);

        let new = [(check("despliegue", "falla"), Some("ok".to_string()))];
        assert_eq!(react(&r, &new, false).unwrap(), 1);
        let run = &runs(&r, 5).unwrap()[0];
        assert!(run.ok && run.source == "reaccion" && run.source_id == x.id, "{run:?}");
        assert!(run.text.contains("Module not found"), "recibe el detalle de la revisión: {}", run.text);
        let id = run.improvement_id.expect("deja una mejora propuesta");
        let mejora = r.improvements(Some("faro")).unwrap().into_iter().find(|m| m.id == id).unwrap();
        assert!(mejora.title.starts_with("Arreglar despliegue"), "{}", mejora.title);
        assert_eq!(mejora.status, "propuesta");

        // Si sube y vuelve a caer, no repite el diagnóstico enseguida.
        assert_eq!(react(&r, &new, false).unwrap(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
