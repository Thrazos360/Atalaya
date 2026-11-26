//! Claude Platform: cuánto se gasta y se usa en la API de Claude.
//!
//! Lee la API de analítica de uso y costo de la organización con una llave de
//! administrador (`sk-ant-admin…`, guardada en el Llavero como `claude`). No cubre
//! lo que se consume con una suscripción de Claude Code o claude.ai: eso no pasa
//! por la API.

use crate::Registry;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::time::Duration;

const BETA: &str = "analytics-usage-cost-2026-08-24";
const VERSION: &str = "2026-10-21";
const CACHE: &str = "claude_summary";
pub const BUDGET: &str = "claude_budget_usd";
const NOTIFIED: &str = "claude_budget_notified";

fn get(path_and_query: &str) -> Result<Value> {
    let key = crate::secrets::get("claude")
        .context("falta la llave de administrador de Claude: agrégala en Conexiones → IA → Claude Platform (consumo)")?;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        .http_status_as_error(false)
        .build()
        .into();
    let mut response = agent
        .get(&format!("https://api.anthropic.com{path_and_query}"))
        .header("x-api-key", &key)
        .header("anthropic-version", VERSION)
        .header("anthropic-beta", BETA)
        .header("User-Agent", "Atalaya")
        .call()
        .context("no se pudo conectar con Claude Platform")?;
    let code = response.status().as_u16();
    let body: Value = response
        .body_mut()
        .read_to_string()
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    if code >= 400 {
        let detail = body["error"]["message"].as_str().unwrap_or("sin detalle");
        match code {
            401 if !detail.contains("Admin API") => {
                bail!("Claude Platform rechazó la llave (error 401): {detail}")
            }
            // Una llave normal de API es válida, pero no puede leer el consumo de la organización.
            401 | 403 | 404 => bail!(
                "la llave guardada no es de administrador (error {code}). Hace falta una llave creada en la consola de Claude en Settings → Admin keys, que empieza con sk-ant-admin; las llaves de la sección API keys no sirven para leer el consumo"
            ),
            _ => bail!("Claude Platform respondió con error {code}: {detail}"),
        }
    }
    Ok(body)
}

/// Todas las páginas de una consulta.
fn rows(path_and_query: &str) -> Result<Vec<Value>> {
    let mut all = Vec::new();
    let mut page: Option<String> = None;
    for _ in 0..20 {
        let url = match &page {
            Some(p) => format!("{path_and_query}&page={p}"),
            None => path_and_query.to_string(),
        };
        let body = get(&url)?;
        all.extend(body["data"].as_array().cloned().unwrap_or_default());
        if body["has_more"] != true {
            break;
        }
        page = body["next_page"].as_str().map(String::from);
        if page.is_none() {
            break;
        }
    }
    Ok(all)
}

/// Los montos llegan en centavos, como texto decimal ("41280.000000").
fn usd(row: &Value) -> f64 {
    row["cost"]["amount"].as_str().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0) / 100.0
}

/// Arma el resumen del mes a partir de las filas de costo y uso por modelo y del costo por día.
fn summarize(cost_by_model: &[Value], usage_by_model: &[Value], cost_by_day: &[Value]) -> Value {
    let mut models: Vec<Value> = cost_by_model
        .iter()
        .map(|row| {
            let model = row["group"]["model"].as_str().unwrap_or("otros").to_string();
            let usage = usage_by_model
                .iter()
                .find(|u| u["group"]["model"].as_str().unwrap_or("otros") == model)
                .map(|u| &u["usage"]);
            let count = |key: &str| usage.and_then(|u| u[key].as_u64()).unwrap_or(0);
            json!({
                "model": model,
                "usd": usd(row),
                "requests": count("requests"),
                "input_tokens": count("uncached_input_tokens") + count("cache_read_input_tokens"),
                "output_tokens": count("output_tokens"),
            })
        })
        .collect();
    models.sort_by(|a, b| b["usd"].as_f64().partial_cmp(&a["usd"].as_f64()).unwrap());
    // `+ 0.0` evita que una suma vacía se muestre como "-0.00".
    let sum = |key: &str| models.iter().filter_map(|m| m[key].as_f64()).sum::<f64>() + 0.0;
    let mut days: Vec<Value> = cost_by_day
        .iter()
        .map(|row| json!({ "day": row["starting_at"].as_str().unwrap_or_default().chars().take(10).collect::<String>(), "usd": usd(row) }))
        .collect();
    days.sort_by(|a, b| a["day"].as_str().cmp(&b["day"].as_str()));
    json!({
        "month_usd": sum("usd"),
        "requests": sum("requests") as u64,
        "output_tokens": sum("output_tokens") as u64,
        "input_tokens": sum("input_tokens") as u64,
        "models": models,
        "days": days,
    })
}

/// Consulta Claude Platform y guarda el resumen del mes en curso.
pub fn refresh(registry: &Registry) -> Result<Value> {
    let (month_start, days_start, today) = registry.utc_marks()?;
    let month = format!("starting_at={month_start}&group_by[]=model&limit=1000");
    let cost = rows(&format!("/v1/organizations/analytics/cost?{month}"))?;
    let usage = rows(&format!("/v1/organizations/analytics/usage?{month}"))?;
    // La serie diaria termina en la medianoche UTC de hoy: el día en curso aún está incompleto.
    let daily = if days_start < today {
        rows(&format!(
            "/v1/organizations/analytics/cost?starting_at={days_start}&ending_at={today}&time_bucket_width=1d&order_by=time&limit=1000"
        ))?
    } else {
        Vec::new()
    };
    let mut summary = summarize(&cost, &usage, &daily);
    summary["fetched_at"] = json!(registry.utc_now()?);
    summary["month"] = json!(month_start.chars().take(7).collect::<String>());
    registry.set_setting(CACHE, &summary.to_string())?;
    Ok(summary)
}

/// Lo último que se leyó, más el presupuesto y si hay llave. No toca la red.
pub fn status(registry: &Registry) -> Result<Value> {
    let summary: Value = registry
        .setting(CACHE)?
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(Value::Null);
    let budget = registry.setting(BUDGET)?.and_then(|b| b.parse::<f64>().ok());
    Ok(json!({
        "connected": crate::secrets::get("claude").is_some(),
        "budget_usd": budget,
        "summary": summary,
        "error": registry.setting("claude_error")?,
    }))
}

/// Ronda del vigía: refresca una vez por hora y avisa al cruzar el 80 % y el 100 % del presupuesto.
pub fn round(registry: &Registry) -> Result<()> {
    if crate::secrets::get("claude").is_none() {
        return Ok(());
    }
    let fresh = registry
        .setting(CACHE)?
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|s| s["fetched_at"].as_str().map(String::from))
        .is_some_and(|at| registry.seconds_since(&at).unwrap_or(f64::MAX) < 3600.0);
    if fresh {
        return Ok(());
    }
    let summary = match refresh(registry) {
        Ok(summary) => {
            registry.set_setting("claude_error", "")?;
            summary
        }
        Err(e) => {
            registry.set_setting("claude_error", &format!("{e:#}"))?;
            return Err(e);
        }
    };
    let Some(budget) = registry.setting(BUDGET)?.and_then(|b| b.parse::<f64>().ok()).filter(|b| *b > 0.0) else {
        return Ok(());
    };
    let spent = summary["month_usd"].as_f64().unwrap_or(0.0);
    let level = if spent >= budget { 100 } else if spent >= budget * 0.8 { 80 } else { 0 };
    // Un aviso por umbral y por mes.
    let mark = format!("{}:{level}", summary["month"].as_str().unwrap_or_default());
    if level > 0 && registry.setting(NOTIFIED)?.as_deref() != Some(mark.as_str()) {
        let title = format!("Claude Platform: {level} % del presupuesto");
        let body = format!("Van US$ {spent:.2} de US$ {budget:.2} este mes.");
        crate::checks::notify(registry, "consumo", &title, &body);
        if crate::mail::configured(registry).unwrap_or(false) {
            let _ = crate::mail::send(registry, &format!("Atalaya: {title}"), &body);
        }
        registry.set_setting(NOTIFIED, &mark)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarizes_cost_and_usage_by_model() {
        let cost = vec![
            json!({ "group": { "model": "claude-haiku-5-5" }, "cost": { "amount": "250.000000" } }),
            json!({ "group": { "model": "claude-opus-5-5" }, "cost": { "amount": "41280.000000" } }),
        ];
        let usage = vec![json!({
            "group": { "model": "claude-opus-5-5" },
            "usage": { "uncached_input_tokens": 1000, "cache_read_input_tokens": 500, "output_tokens": 300, "requests": 42 }
        })];
        let days = vec![
            json!({ "starting_at": "2026-11-21T00:00:00Z", "cost": { "amount": "100.0" } }),
            json!({ "starting_at": "2026-11-20T00:00:00Z", "cost": { "amount": "50.0" } }),
        ];
        let s = summarize(&cost, &usage, &days);
        assert_eq!(s["month_usd"].as_f64(), Some(415.3));
        assert_eq!(s["models"][0]["model"], "claude-opus-5-5");
        assert_eq!(s["models"][0]["requests"], 42);
        assert_eq!(s["models"][0]["input_tokens"], 1500);
        assert_eq!(s["requests"], 42);
        assert_eq!(s["days"][0]["day"], "2026-11-20");
        assert_eq!(s["days"][1]["usd"].as_f64(), Some(1.0));
    }
}
