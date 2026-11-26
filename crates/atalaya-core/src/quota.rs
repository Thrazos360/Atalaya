//! IA que se quedaron sin cuota: el plan gratis de Cursor, la cuota semanal de Antigravity,
//! el límite de Claude Code o un 429 de un servicio por API.
//!
//! Cuando una llamada falla con un aviso de ese tipo, la IA queda en pausa hasta que se
//! renueve: si el aviso dice cuándo («Resets in 128h2m47s»), hasta ese momento; si no,
//! unas horas, y entonces se vuelve a probar. Mientras tanto el orquestador la salta y le
//! pasa el trabajo a la siguiente. Una llamada que sale bien la saca de la pausa.

use crate::Registry;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

/// Las pausas de cada conexión, en JSON: `{id: {until, reason}}`.
const SETTING: &str = "sin_cuota";

/// Sin fecha en el aviso: se vuelve a probar después de esto.
const QUOTA_PAUSE: u64 = 3 * 3600;
/// Un «demasiados pedidos» suele pasar pronto.
const RATE_PAUSE: u64 = 10 * 60;
/// Ningún aviso pausa más que esto, por si el texto se leyó mal.
const MAX_PAUSE: u64 = 8 * 24 * 3600;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pause {
    /// Segundos desde 1970 (UTC) en que se vuelve a intentar.
    pub until: u64,
    /// La línea del aviso, tal como la dio la IA.
    pub reason: String,
}

/// Un aviso de cuota agotada: cuánto esperar y la línea que lo dice.
#[derive(Debug, Clone, PartialEq)]
pub struct Exhausted {
    pub wait_secs: u64,
    pub line: String,
}

const QUOTA_WORDS: &[&str] = &[
    "quota",
    "resource_exhausted",
    "usage limit",
    "hit your limit",
    "limit reached",
    "out of credits",
    "credit balance is too low",
    "insufficient_quota",
    "upgrade your plan",
    "upgrade your subscription",
];
const RATE_WORDS: &[&str] = &["rate limit", "rate_limit", "ratelimit", "too many requests", "429"];

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Si el texto es un aviso de cuota agotada o de demasiados pedidos.
pub fn detect(text: &str) -> Option<Exhausted> {
    let lower = text.to_lowercase();
    let quota = QUOTA_WORDS.iter().any(|w| lower.contains(w));
    let rate = RATE_WORDS.iter().any(|w| lower.contains(w));
    if !quota && !rate {
        return None;
    }
    let default = if quota { QUOTA_PAUSE } else { RATE_PAUSE };
    let wait = reset_in(&lower).unwrap_or(default).clamp(60, MAX_PAUSE);
    Some(Exhausted { wait_secs: wait, line: text.trim().to_string() })
}

/// Lee «resets in 128h2m47s», «try again in 5 minutes» o «retry after 30s».
fn reset_in(lower: &str) -> Option<u64> {
    for key in ["resets in", "reset in", "try again in", "retry in", "retry after", "try again after", "available in"] {
        if let Some(pos) = lower.find(key) {
            if let Some(secs) = duration(&lower[pos + key.len()..]) {
                return Some(secs);
            }
        }
    }
    None
}

/// Suma números con unidad al comienzo del texto: «128h2m47s», «2 hours 5 min», «1d».
fn duration(text: &str) -> Option<u64> {
    let mut total = 0u64;
    let mut found = false;
    let mut rest = text.trim_start();
    loop {
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() {
            break;
        }
        let n: u64 = digits.parse().ok()?;
        let after = rest[digits.len()..].trim_start();
        let unit: String = after.chars().take_while(|c| c.is_ascii_alphabetic()).collect();
        let factor = match unit.as_str() {
            "d" | "day" | "days" => 86_400,
            "h" | "hr" | "hrs" | "hour" | "hours" => 3600,
            "m" | "min" | "mins" | "minute" | "minutes" => 60,
            "s" | "sec" | "secs" | "second" | "seconds" => 1,
            _ => break,
        };
        total += n * factor;
        found = true;
        rest = after[unit.len()..].trim_start_matches([' ', ',']).trim_start_matches("and ").trim_start();
    }
    found.then_some(total)
}

/// La línea que explica por qué falló un agente: el aviso de cuota si lo hay entre lo
/// último que escribió; si no, la última línea que habla de un error, o la última.
pub fn failure_line(output: &str) -> Option<String> {
    let lines: Vec<&str> = output.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let tail = &lines[lines.len().saturating_sub(15)..];
    let line = tail
        .iter()
        .rev()
        .find(|l| detect(l).is_some())
        .or_else(|| tail.iter().rev().find(|l| l.to_lowercase().contains("error")))
        .or_else(|| tail.last())?;
    let line = readable(line);
    let clipped: String = line.chars().take(300).collect();
    Some(if clipped.len() < line.len() { format!("{clipped}…") } else { clipped })
}

/// Si la línea trae un error en JSON (`AGY_ERROR: {"short_error": …}`), solo su mensaje.
fn readable(line: &str) -> String {
    let json = line.find('{').and_then(|i| serde_json::from_str::<serde_json::Value>(&line[i..]).ok());
    let message = json.and_then(|v| {
        ["/short_error", "/message", "/error/message", "/error"]
            .iter()
            .find_map(|ptr| v.pointer(ptr).and_then(|m| m.as_str()).map(String::from))
    });
    message.unwrap_or_else(|| line.to_string())
}

/// El texto de error que queda anotado para un agente que terminó mal.
pub fn agent_error(output: &str, timed_out: bool) -> String {
    if timed_out {
        return "se acabó el tiempo".into();
    }
    match failure_line(output) {
        Some(line) => format!("terminó con error: {line}"),
        None => "terminó con error".into(),
    }
}

fn load(registry: &Registry) -> Result<BTreeMap<String, Pause>> {
    Ok(registry.setting(SETTING)?.and_then(|v| serde_json::from_str(&v).ok()).unwrap_or_default())
}

fn store(registry: &Registry, all: &BTreeMap<String, Pause>) -> Result<()> {
    registry.set_setting(SETTING, &serde_json::to_string(all)?)
}

/// La pausa vigente de una conexión, si tiene.
pub fn paused(registry: &Registry, connection_id: &str) -> Result<Option<Pause>> {
    Ok(load(registry)?.remove(connection_id).filter(|p| p.until > now()))
}

/// Toma nota del resultado de una llamada: si salió bien, quita la pausa; si falló por
/// cuota, la pone (y lo cuenta en la actividad la primera vez). Devuelve la pausa nueva.
pub fn note(registry: &Registry, connection_id: &str, name: &str, error: Option<&str>) -> Result<Option<Pause>> {
    let mut all = load(registry)?;
    let Some(error) = error else {
        if all.remove(connection_id).is_some() {
            store(registry, &all)?;
        }
        return Ok(None);
    };
    let Some(found) = detect(error) else { return Ok(None) };
    let already = all.get(connection_id).is_some_and(|p| p.until > now());
    let line = found.line.strip_prefix("terminó con error: ").unwrap_or(&found.line);
    let reason: String = line.chars().take(300).collect();
    let pause = Pause { until: now() + found.wait_secs, reason };
    all.insert(connection_id.to_string(), pause.clone());
    store(registry, &all)?;
    if !already {
        let until = local_time(registry, pause.until)?;
        registry.record_event("atalaya", &format!("{name} se quedó sin cuota; Atalaya la salta hasta el {until}"), "consumo")?;
    }
    Ok(Some(pause))
}

/// Quita la pausa a mano, para volver a intentar ya.
pub fn clear(registry: &Registry, connection_id: &str) -> Result<()> {
    note(registry, connection_id, "", None).map(|_| ())
}

/// «30-11 18:52» en la hora de este Mac.
pub fn local_time(registry: &Registry, epoch: u64) -> Result<String> {
    Ok(registry.conn().query_row("SELECT strftime('%d-%m %H:%M', ?1, 'unixepoch', 'localtime')", [epoch as i64], |r| r.get(0))?)
}

/// El motivo para saltar a esta IA, si está en pausa.
pub fn unavailable(registry: &Registry, connection_id: &str) -> Result<Option<String>> {
    Ok(match paused(registry, connection_id)? {
        Some(p) => Some(format!("sin cuota hasta el {}", local_time(registry, p.until)?)),
        None => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_quota_notices_and_their_reset() {
        let agy = "error: Individual quota reached. Please upgrade your subscription to increase your limits. Resets in 128h2m47s.";
        let e = detect(agy).unwrap();
        assert_eq!(e.wait_secs, 128 * 3600 + 2 * 60 + 47);
        let cursor = "ActionRequiredError: You've hit your usage limit Get Cursor Pro for more Agent usage, unlimited Tab, and more.";
        assert_eq!(detect(cursor).unwrap().wait_secs, QUOTA_PAUSE);
        assert_eq!(detect("HTTP 429 Too Many Requests, try again in 2 minutes").unwrap().wait_secs, 120);
        assert_eq!(detect("429 too many requests").unwrap().wait_secs, RATE_PAUSE);
        assert_eq!(detect("quota exceeded, resets in 1 day 2 hours").unwrap().wait_secs, 86_400 + 7200);
        assert!(detect("terminó con error: no se encontró el archivo").is_none());
        // Nunca más de unos días, aunque el texto diga otra cosa.
        assert_eq!(detect("quota reached. Resets in 9999h").unwrap().wait_secs, MAX_PAUSE);
    }

    #[test]
    fn picks_the_line_that_explains_the_failure() {
        let out = "pensando…\n\nerror: Individual quota reached. Resets in 2h.\nAGY_ERROR: {\"status\":\"RESOURCE_EXHAUSTED\"}\n";
        // Las dos hablan de cuota; vale la última, y de su JSON solo el mensaje.
        let out = format!("{out}AGY_ERROR: {{\"short_error\":\"RESOURCE_EXHAUSTED (code 429): Individual quota reached.\"}}\n");
        assert_eq!(failure_line(&out).unwrap(), "RESOURCE_EXHAUSTED (code 429): Individual quota reached.");
        let out = "línea 1\nError: EPERM: process.cwd failed\n    at foo (x.js:1)\nNode.js v26\n";
        assert_eq!(failure_line(out).unwrap(), "Error: EPERM: process.cwd failed");
        assert_eq!(agent_error("", true), "se acabó el tiempo");
        assert_eq!(agent_error("  \n", false), "terminó con error");
    }

    #[test]
    fn pauses_until_it_resets_and_a_good_call_lifts_it() {
        let dir = std::env::temp_dir().join(format!("atalaya-cuota-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let r = Registry::open(&dir.join("t.db")).unwrap();
        assert!(note(&r, "cursor", "Cursor", Some("terminó con error: no compila")).unwrap().is_none());
        assert!(paused(&r, "cursor").unwrap().is_none());
        let p = note(&r, "cursor", "Cursor", Some("terminó con error: You've hit your usage limit")).unwrap().unwrap();
        assert!(p.until > now() + QUOTA_PAUSE - 5);
        assert!(unavailable(&r, "cursor").unwrap().unwrap().starts_with("sin cuota hasta el "));
        // Otra falla igual no repite el aviso en la actividad.
        note(&r, "cursor", "Cursor", Some("You've hit your usage limit")).unwrap();
        let events: i64 =
            r.conn().query_row("SELECT count(*) FROM events WHERE what LIKE 'Cursor se quedó sin cuota%'", [], |x| x.get(0)).unwrap();
        assert_eq!(events, 1);
        // Una pausa vencida no cuenta.
        let mut all = load(&r).unwrap();
        all.get_mut("cursor").unwrap().until = now() - 1;
        store(&r, &all).unwrap();
        assert!(paused(&r, "cursor").unwrap().is_none());
        note(&r, "cursor", "Cursor", Some("usage limit")).unwrap();
        note(&r, "cursor", "Cursor", None).unwrap();
        assert!(paused(&r, "cursor").unwrap().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
