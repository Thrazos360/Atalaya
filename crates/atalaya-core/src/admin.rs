//! Puerta de administración: los controles que cada app le ofrece a Atalaya.
//!
//! Atalaya no escribe en la base de ninguna app. Cada una expone `GET /api/atalaya`
//! con sus controles y valores actuales, y `POST /api/atalaya` para aplicar un cambio
//! a la vez, protegidos por una llave propia de esa app (protocolo 1, ver
//! `docs/puerta-admin.md`). La app valida y aplica el cambio con su propia lógica y
//! lo deja en su propia auditoría.
//!
//! La llave la genera Atalaya, vive en el Llavero (cuenta `admin-<slug>`) y se guarda
//! en el proyecto de Vercel como `ATALAYA_ADMIN_KEY`.

use crate::Registry;
use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde_json::{Value, json};
use std::time::Duration;

/// Versión del protocolo que entiende esta versión de Atalaya.
pub const PROTOCOL: u64 = 1;
/// Variable de entorno donde cada app espera su llave.
pub const ENV_KEY: &str = "ATALAYA_ADMIN_KEY";

fn url_setting(slug: &str) -> String {
    format!("admin_url:{slug}")
}

fn key_account(slug: &str) -> String {
    format!("admin-{slug}")
}

/// `admin.tuapp.com` → `https://admin.tuapp.com/api/atalaya`.
pub fn normalize_url(raw: &str) -> Result<String> {
    let raw = raw.trim().trim_end_matches('/');
    if raw.is_empty() {
        bail!("falta la dirección de la app");
    }
    let base = if raw.starts_with("http://") || raw.starts_with("https://") {
        raw.to_string()
    } else {
        format!("https://{raw}")
    };
    let local = base.starts_with("http://localhost") || base.starts_with("http://127.0.0.1");
    if base.starts_with("http://") && !local {
        bail!("la puerta tiene que ir por https: la llave viaja en cada pedido");
    }
    Ok(if base.ends_with("/api/atalaya") { base } else { format!("{base}/api/atalaya") })
}

#[derive(Serialize, Debug, Clone)]
pub struct Door {
    /// Dónde está la puerta, si ya se conectó.
    pub url: Option<String>,
    /// Si el Llavero tiene la llave de esta app.
    pub has_key: bool,
}

pub fn door(registry: &Registry, slug: &str) -> Result<Door> {
    Ok(Door {
        url: registry.setting(&url_setting(slug))?,
        has_key: crate::secrets::get(&key_account(slug)).is_some(),
    })
}

/// 32 bytes al azar del sistema, en hexadecimal.
fn new_key() -> Result<String> {
    use std::io::Read;
    let mut bytes = [0u8; 32];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .context("no se pudo generar la llave")?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

#[derive(Serialize, Debug)]
pub struct Connected {
    pub url: String,
    /// La llave se guardó en el proyecto de Vercel.
    pub vercel: bool,
    /// Se generó una llave nueva (la app necesita un despliegue para tomarla).
    pub new_key: bool,
    pub note: String,
}

/// Deja lista la conexión: dirección, llave en el Llavero y, si el proyecto está en
/// Vercel, la llave como variable de producción. Reusa la llave existente salvo que
/// se pida rotarla, para que conectar dos veces no rompa nada.
pub fn connect(registry: &Registry, slug: &str, url: &str, rotate: bool) -> Result<Connected> {
    let project = registry
        .get(slug)?
        .with_context(|| format!("no existe el proyecto '{slug}'"))?;
    let url = normalize_url(url)?;
    let account = key_account(slug);
    let existing = crate::secrets::get(&account).filter(|_| !rotate);
    let fresh = existing.is_none();
    let key = match existing {
        Some(key) => key,
        None => {
            let key = new_key()?;
            if !crate::secrets::set(&account, &key)? {
                bail!("no se pudo guardar la llave en el Llavero");
            }
            key
        }
    };
    registry.set_setting(&url_setting(slug), &url)?;

    let services = crate::inspect::project_services(&project);
    let vercel = match services.vercel_project_id {
        Some(project_id) => {
            let team = services.vercel_team_id.filter(|t| t.starts_with("team_"));
            save_in_vercel(&project_id, team.as_deref(), &key)?;
            true
        }
        None => false,
    };
    let note = match (vercel, fresh) {
        (true, true) => "Llave nueva guardada en el Llavero y en Vercel. La app la toma en su próximo despliegue.",
        (true, false) => "Se reusó la llave del Llavero y se confirmó en Vercel.",
        (false, _) => "El proyecto no está en Vercel: pon la llave como ATALAYA_ADMIN_KEY donde corra la app (atalaya admin key).",
    }
    .to_string();
    registry.record_event(slug, &format!("puerta de administración conectada en {url}"), "terminal")?;
    Ok(Connected { url, vercel, new_key: fresh, note })
}

/// Variable de producción con la llave. Tipo "sensitive": nadie puede volver a leer su
/// valor en el panel ni en la API de Vercel, solo la app al desplegarse.
fn env_body(key: &str) -> Value {
    json!({
        "key": ENV_KEY,
        "value": key,
        "type": "sensitive",
        "target": ["production"],
    })
}

/// Guarda la llave como variable sensible. Vercel no deja convertir una variable
/// existente a sensible, así que la que quedó de una conexión anterior con otro tipo
/// se borra y se vuelve a crear con el mismo valor. Un despliegue ya publicado conserva
/// la suya: el cambio solo se nota en el próximo.
fn save_in_vercel(project_id: &str, team: Option<&str>, key: &str) -> Result<()> {
    use crate::flags::{expect_ok, vercel};
    let listed = expect_ok(
        "leer las variables del proyecto",
        vercel("GET", &format!("/v10/projects/{project_id}/env"), team, None)?,
    )?;
    for id in stale_ids(&listed) {
        expect_ok(
            "reemplazar la llave anterior",
            vercel("DELETE", &format!("/v9/projects/{project_id}/env/{id}"), team, None)?,
        )?;
    }
    expect_ok(
        "guardar la llave de la puerta",
        vercel(
            "POST",
            &format!("/v10/projects/{project_id}/env?upsert=true"),
            team,
            Some(&env_body(key)),
        )?,
    )?;
    Ok(())
}

/// Las variables con la llave que no son sensibles.
fn stale_ids(listed: &Value) -> Vec<String> {
    listed["envs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|e| e["key"] == ENV_KEY && e["type"] != "sensitive")
        .filter_map(|e| e["id"].as_str().map(str::to_string))
        .collect()
}

/// La llave, para ponerla a mano donde corra una app que no está en Vercel.
pub fn key(slug: &str) -> Option<String> {
    crate::secrets::get(&key_account(slug))
}

fn call(registry: &Registry, slug: &str, body: Option<&Value>) -> Result<Value> {
    let door = door(registry, slug)?;
    let url = door
        .url
        .with_context(|| format!("'{slug}' no tiene puerta de administración conectada"))?;
    let key = crate::secrets::get(&key_account(slug))
        .with_context(|| format!("falta la llave de la puerta de '{slug}' en el Llavero"))?;
    // Sin seguir redirecciones: una app sin puerta suele mandar a su página de ingreso,
    // y la llave no debe viajar a otra dirección.
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        .http_status_as_error(false)
        .max_redirects(0)
        .build()
        .into();
    let auth = format!("Bearer {key}");
    let mut response = match body {
        None => agent.get(&url).header("Authorization", &auth).call(),
        Some(body) => agent
            .post(&url)
            .header("Authorization", &auth)
            .header("Content-Type", "application/json")
            .send(body.to_string()),
    }
    .with_context(|| format!("no se pudo llegar a {url}"))?;
    let code = response.status().as_u16();
    let text = response.body_mut().read_to_string().unwrap_or_default();
    let value: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    match code {
        200..=299 if value.is_object() => Ok(value),
        200..=399 | 404 => bail!(
            "la app todavía no tiene la puerta abierta: falta desplegar la versión con /api/atalaya o la variable {ENV_KEY}"
        ),
        401 => bail!("la app no reconoce la llave: vuelve a conectar la puerta y despliega la app"),
        // Un cambio rechazado: la app ya explica por qué, en palabras simples.
        400 => bail!("{}", value["error"].as_str().unwrap_or("la app rechazó el pedido")),
        _ => {
            let detail = value["error"].as_str().unwrap_or("sin detalle");
            bail!("la app respondió error {code}: {detail}")
        }
    }
}

/// Los controles de la app, tal como ella los describe.
pub fn schema(registry: &Registry, slug: &str) -> Result<Value> {
    let value = call(registry, slug, None)?;
    let protocol = value["protocol"].as_u64().unwrap_or(0);
    if protocol != PROTOCOL {
        bail!("la app habla el protocolo {protocol} y esta versión de Atalaya entiende el {PROTOCOL}");
    }
    Ok(value)
}

/// Manda un cambio (`{section, item, key, value}`) o un alta (`{section, create}`).
/// Devuelve el mensaje de la app y lo deja en la historia del proyecto.
pub fn apply(registry: &Registry, slug: &str, change: &Value, source: &str) -> Result<String> {
    let section = change["section"].as_str().context("falta la sección")?;
    let response = call(registry, slug, Some(change))?;
    if response["ok"].as_bool() != Some(true) {
        bail!("{}", response["error"].as_str().unwrap_or("la app rechazó el cambio"));
    }
    let message = response["message"].as_str().unwrap_or("Cambio aplicado.").to_string();
    registry.record_event(slug, &format!("administración · {section}: {message}"), source)?;
    Ok(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_are_normalized_and_must_be_https() {
        assert_eq!(normalize_url("admin.tuapp.com").unwrap(), "https://admin.tuapp.com/api/atalaya");
        assert_eq!(
            normalize_url("https://admin.tuapp.com/api/atalaya/").unwrap(),
            "https://admin.tuapp.com/api/atalaya"
        );
        assert_eq!(normalize_url("http://localhost:3199").unwrap(), "http://localhost:3199/api/atalaya");
        assert!(normalize_url("http://admin.tuapp.com").is_err());
        assert!(normalize_url("  ").is_err());
    }

    #[test]
    fn keys_are_long_and_random() {
        let (a, b) = (new_key().unwrap(), new_key().unwrap());
        assert_eq!(a.len(), 64);
        assert_ne!(a, b);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn the_key_goes_to_vercel_as_sensitive_production_only() {
        let body = env_body("abc");
        assert_eq!(body["type"], "sensitive");
        assert_eq!(body["target"], json!(["production"]));
        assert_eq!(body["key"], ENV_KEY);
    }

    #[test]
    fn only_non_sensitive_copies_of_the_key_are_replaced() {
        let listed = json!({ "envs": [
            { "id": "a", "key": ENV_KEY, "type": "encrypted" },
            { "id": "b", "key": ENV_KEY, "type": "sensitive" },
            { "id": "c", "key": "OTRA", "type": "encrypted" },
        ]});
        assert_eq!(stale_ids(&listed), vec!["a".to_string()]);
        assert!(stale_ids(&json!({})).is_empty());
    }
}
