//! Lo que Atalaya sabe hacer con cada proveedor de una conexión: probarla, revisar lo
//! que el proyecto tiene en ella y, en las bases, respaldarla.

pub mod database;
pub mod forge;
pub mod hosting;

use crate::Registry;
use crate::connections::Connection;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::time::Duration;

/// (estado, resumen, detalles), igual que las revisiones de `checks`.
pub type Outcome = (&'static str, String, Value);

/// Petición HTTP con cabeceras propias. Devuelve el código y el cuerpo como JSON
/// (`Null` si no era JSON).
pub(crate) fn http(
    method: &str,
    url: &str,
    headers: &[(&str, String)],
    body: Option<&Value>,
    timeout_secs: u64,
) -> Result<(u16, Value)> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(timeout_secs)))
        .http_status_as_error(false)
        .user_agent("Atalaya")
        .build()
        .into();
    let mut response = match method {
        "GET" => {
            let mut req = agent.get(url);
            for (k, v) in headers {
                req = req.header(*k, v);
            }
            req.call()
        }
        "POST" => {
            let mut req = agent.post(url).header("Content-Type", "application/json");
            for (k, v) in headers {
                req = req.header(*k, v);
            }
            req.send(body.map(Value::to_string).unwrap_or_default())
        }
        other => bail!("método no soportado: {other}"),
    }
    .with_context(|| format!("no se pudo conectar con {}", host_of(url)))?;
    let code = response.status().as_u16();
    let text = response.body_mut().with_config().limit(64 * 1024 * 1024).read_to_string().unwrap_or_default();
    Ok((code, serde_json::from_str(&text).unwrap_or(Value::Null)))
}

/// El dominio de una URL, para los mensajes.
pub(crate) fn host_of(url: &str) -> String {
    url.split("://").nth(1).unwrap_or(url).split(['/', '?']).next().unwrap_or(url).to_string()
}

/// Mensaje de error de una API, venga como venga.
pub(crate) fn api_error(body: &Value) -> String {
    for key in ["message", "error_description", "error"] {
        match &body[key] {
            Value::String(s) => return s.clone(),
            Value::Object(o) => {
                if let Some(Value::String(s)) = o.get("message") {
                    return s.clone();
                }
            }
            _ => {}
        }
    }
    "sin detalle".to_string()
}

/// Falla con un mensaje claro si la respuesta no fue 2xx.
pub(crate) fn expect_2xx(service: &str, (code, body): (u16, Value)) -> Result<Value> {
    match code {
        200..=299 => Ok(body),
        401 | 403 => bail!("{service} rechazó la llave (error {code}): {}", api_error(&body)),
        404 => bail!("{service} no encontró lo pedido (error 404): {}", api_error(&body)),
        _ => bail!("{service} respondió con error {code}: {}", api_error(&body)),
    }
}

fn need_secret(c: &Connection) -> Result<String> {
    c.secret().with_context(|| format!("la conexión «{}» no tiene llave guardada", c.name))
}

/// Prueba una conexión. Devuelve una frase que confirma qué se encontró.
pub fn test(registry: &Registry, c: &Connection) -> Result<String> {
    match c.provider.as_str() {
        "github" => {
            let repos = crate::github::repos()?;
            Ok(format!("Conectado: {} repos visibles", repos.len()))
        }
        "gitlab" | "gitea" => forge::test(c),
        "vercel" => hosting::vercel_test(c),
        "netlify" => hosting::netlify_test(c),
        "servidor" => hosting::server_test(c),
        "supabase" => {
            let token = need_secret(c)?;
            let body = expect_2xx(
                "Supabase",
                http("GET", "https://api.supabase.com/v1/projects", &[("Authorization", format!("Bearer {token}"))], None, 30)?,
            )?;
            let names: Vec<&str> =
                body.as_array().into_iter().flatten().filter_map(|p| p["name"].as_str()).take(5).collect();
            Ok(format!(
                "Conectado: {} proyectos{}",
                body.as_array().map_or(0, Vec::len),
                if names.is_empty() { String::new() } else { format!(" ({})", names.join(", ")) }
            ))
        }
        "firebase" => {
            let saved = need_secret(c)?;
            let account = crate::firebase::account_from_saved(&saved).context("la cuenta de servicio guardada no se puede leer")?;
            crate::firebase::test_account(&account)
        }
        "postgres" | "mysql" | "sqlite" => database::test(c),
        "correo" => {
            crate::mail::send(registry, "Prueba de Atalaya", "Si lees esto, el correo de Atalaya funciona.")?;
            Ok("Correo de prueba enviado".to_string())
        }
        "telegram" => {
            if crate::checks::telegram(registry, "Prueba de Atalaya: los avisos llegan bien.") {
                Ok("Mensaje de prueba enviado".to_string())
            } else {
                bail!("Telegram no aceptó el mensaje: revisa el token del bot y el chat")
            }
        }
        "anthropic-admin" => {
            let summary = crate::claude::refresh(registry)?;
            Ok(format!(
                "Conectado: US$ {:.2} gastados este mes",
                summary["month_usd"].as_f64().unwrap_or(0.0)
            ))
        }
        "anthropic" | "openai" | "gemini" | "openai-compatible" | "ollama" => crate::ai::test(registry, c),
        p if crate::connections::provider(p).is_some_and(|p| p.kind == "agente") => crate::agents::test(c),
        other => bail!("no se sabe probar el proveedor '{other}'"),
    }
}

/// Junta varias revisiones en una: el peor estado manda y el resumen las nombra a todas.
pub(crate) fn combine(parts: Vec<(String, Outcome)>) -> Option<Outcome> {
    if parts.is_empty() {
        return None;
    }
    let rank = |s: &str| match s {
        crate::checks::FAIL => 2,
        crate::checks::WARN => 1,
        _ => 0,
    };
    let worst = parts.iter().map(|(_, (s, _, _))| *s).max_by_key(|s| rank(s)).unwrap_or(crate::checks::OK);
    if parts.len() == 1 {
        let (_, outcome) = parts.into_iter().next()?;
        return Some(outcome);
    }
    let summary = parts.iter().map(|(name, (_, s, _))| format!("{name}: {s}")).collect::<Vec<_>>().join(" · ");
    let details = Value::Array(
        parts.into_iter().map(|(name, (s, sum, d))| json!({ "conexion": name, "estado": s, "resumen": sum, "detalles": d })).collect(),
    );
    Some((worst, summary, details))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checks::{FAIL, OK, WARN};

    #[test]
    fn combine_keeps_the_worst_status() {
        assert!(combine(vec![]).is_none());
        let one = combine(vec![("a".into(), (OK, "bien".into(), Value::Null))]).unwrap();
        assert_eq!((one.0, one.1.as_str()), (OK, "bien"));
        let many = combine(vec![
            ("a".into(), (OK, "bien".into(), Value::Null)),
            ("b".into(), (FAIL, "caída".into(), Value::Null)),
            ("c".into(), (WARN, "lenta".into(), Value::Null)),
        ])
        .unwrap();
        assert_eq!(many.0, FAIL);
        assert_eq!(many.1, "a: bien · b: caída · c: lenta");
    }

    #[test]
    fn reads_api_errors() {
        assert_eq!(api_error(&json!({ "message": "x" })), "x");
        assert_eq!(api_error(&json!({ "error": { "message": "y" } })), "y");
        assert_eq!(api_error(&json!({ "error": "z" })), "z");
        assert_eq!(api_error(&Value::Null), "sin detalle");
        assert_eq!(host_of("https://api.x.com/v1?a=b"), "api.x.com");
    }
}
