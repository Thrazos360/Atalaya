//! Consultas de solo lectura a una base de Supabase, por su API de gestión.

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::time::Duration;

/// Ejecuta una consulta con la llave predeterminada de Supabase.
pub fn query(reference: &str, sql: &str) -> Result<Vec<Value>> {
    let token = crate::secrets::get("supabase")
        .context("falta la llave de Supabase: agrégala en Conexiones")?;
    query_with(&token, reference, sql)
}

/// Ejecuta una consulta en modo de solo lectura y devuelve sus filas.
pub fn query_with(token: &str, reference: &str, sql: &str) -> Result<Vec<Value>> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(120)))
        .http_status_as_error(false)
        .build()
        .into();
    let body = json!({ "query": sql, "read_only": true });
    let mut response = agent
        .post(&format!("https://api.supabase.com/v1/projects/{reference}/database/query"))
        .header("Authorization", &format!("Bearer {token}"))
        .header("Content-Type", "application/json")
        .send(body.to_string())
        .context("no se pudo conectar con Supabase")?;
    let code = response.status().as_u16();
    let text = response.body_mut().with_config().limit(512 * 1024 * 1024).read_to_string().unwrap_or_default();
    let parsed: Value = serde_json::from_str(&text).unwrap_or_default();
    if code >= 400 {
        bail!("Supabase rechazó la consulta (error {code}): {}", parsed["message"].as_str().unwrap_or("sin detalle"));
    }
    Ok(parsed.as_array().cloned().unwrap_or_default())
}

/// Tamaño de la base de datos en bytes.
pub fn database_bytes_with(token: &str, reference: &str) -> Result<u64> {
    let rows = query_with(token, reference, "select pg_database_size(current_database()) as bytes")?;
    rows.first().and_then(|r| r["bytes"].as_u64()).context("Supabase no devolvió el tamaño")
}
