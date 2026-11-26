//! Interruptores de funciones. Viven en un almacén Global Config (Edge Config) de Vercel:
//! una entrada por proyecto, con la forma `{ "nombre": { "on": true, "desc": "…" } }`.
//! Las apps lo leen al instante, sin volver a desplegar, y Atalaya lo escribe desde
//! la ventana, la terminal o el MCP. Como la verdad está en Vercel, cualquier nodo
//! de Atalaya (Mac o Proxmox) ve lo mismo.

use crate::Registry;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::time::Duration;

pub const STORE_NAME: &str = "atalaya-flags";
const STORE_ID: &str = "flags_store_id";
const TEAM_ID: &str = "flags_team_id";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Flag {
    pub on: bool,
    #[serde(default)]
    pub desc: String,
    /// Texto o valor que acompaña al interruptor (por ejemplo, el mensaje de un aviso).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub value: String,
}

pub type Flags = BTreeMap<String, Flag>;

/// Llamada a la API de Vercel con la llave guardada. Devuelve código HTTP y cuerpo JSON.
pub(crate) fn vercel(
    method: &str,
    path: &str,
    team: Option<&str>,
    body: Option<&Value>,
) -> Result<(u16, Value)> {
    let token = crate::connections::token_for_default("vercel")
        .context("falta la llave de Vercel: agrégala en Conexiones")?;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        .http_status_as_error(false)
        .build()
        .into();
    let mut url = format!("https://api.vercel.com{path}");
    if let Some(team) = team {
        url.push_str(if path.contains('?') { "&" } else { "?" });
        url.push_str(&format!("teamId={team}"));
    }
    let auth = format!("Bearer {token}");
    let mut response = match (method, body) {
        ("GET", _) => agent.get(&url).header("Authorization", &auth).call(),
        ("DELETE", _) => agent.delete(&url).header("Authorization", &auth).call(),
        (_, body) => {
            let request = match method {
                "POST" => agent.post(&url),
                "PATCH" => agent.patch(&url),
                other => bail!("método no soportado: {other}"),
            };
            request
                .header("Authorization", &auth)
                .header("Content-Type", "application/json")
                .send(body.map(Value::to_string).unwrap_or_default())
        }
    }
    .context("no se pudo conectar con Vercel")?;
    let parsed = response
        .body_mut()
        .read_to_string()
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    Ok((response.status().as_u16(), parsed))
}

pub(crate) fn expect_ok(what: &str, (code, body): (u16, Value)) -> Result<Value> {
    if code >= 400 {
        let message = body["error"]["message"].as_str().unwrap_or("sin detalle");
        bail!("Vercel rechazó {what} (error {code}): {message}");
    }
    Ok(body)
}

/// Equipo de Vercel donde vive el almacén: el elegido en la conexión de Vercel, el ya
/// guardado o, si no hay ninguno, el del primer proyecto que tenga uno.
fn team(registry: &Registry) -> Result<Option<String>> {
    if let Some(team) = crate::connections::default_for(registry, "vercel")?.and_then(|c| c.config_str("team_id")) {
        return Ok(Some(team));
    }
    if let Some(team) = registry.setting(TEAM_ID)? {
        return Ok(Some(team));
    }
    for project in registry.list()? {
        let team = crate::inspect::project_services(&project).vercel_team_id;
        if let Some(team) = team.filter(|t| t.starts_with("team_")) {
            registry.set_setting(TEAM_ID, &team)?;
            return Ok(Some(team));
        }
    }
    Ok(None)
}

/// Devuelve el ID del almacén; lo busca en Vercel por nombre y, si no existe, lo crea.
pub fn store(registry: &Registry) -> Result<String> {
    if let Some(id) = registry.setting(STORE_ID)? {
        return Ok(id);
    }
    let team = team(registry)?;
    let team = team.as_deref();
    let existing = expect_ok("listar los almacenes", vercel("GET", "/v1/edge-config", team, None)?)?;
    let found = existing
        .as_array()
        .and_then(|list| list.iter().find(|s| s["slug"] == STORE_NAME))
        .and_then(|s| s["id"].as_str().map(String::from));
    let id = match found {
        Some(id) => id,
        None => {
            let body = json!({ "slug": STORE_NAME });
            let created = expect_ok(
                "crear el almacén",
                vercel("POST", "/v1/edge-config", team, Some(&body))?,
            )?;
            created["id"]
                .as_str()
                .context("Vercel no devolvió el ID del almacén")?
                .to_string()
        }
    };
    registry.set_setting(STORE_ID, &id)?;
    Ok(id)
}

fn known_project(registry: &Registry, slug: &str) -> Result<()> {
    registry
        .get(slug)?
        .map(|_| ())
        .with_context(|| format!("no existe el proyecto '{slug}'"))
}

/// Interruptores de un proyecto, leídos de Vercel.
pub fn list(registry: &Registry, slug: &str) -> Result<Flags> {
    known_project(registry, slug)?;
    let id = store(registry)?;
    let items = expect_ok(
        "leer los interruptores",
        vercel("GET", &format!("/v1/edge-config/{id}/items"), team(registry)?.as_deref(), None)?,
    )?;
    let value = items
        .as_array()
        .and_then(|list| list.iter().find(|item| item["key"] == slug))
        .map(|item| item["value"].clone())
        .unwrap_or_else(|| json!({}));
    Ok(serde_json::from_value(value).unwrap_or_default())
}

fn save(registry: &Registry, slug: &str, flags: &Flags) -> Result<()> {
    let id = store(registry)?;
    let body = json!({ "items": [{ "operation": "upsert", "key": slug, "value": flags }] });
    expect_ok(
        "guardar los interruptores",
        vercel(
            "PATCH",
            &format!("/v1/edge-config/{id}/items"),
            team(registry)?.as_deref(),
            Some(&body),
        )?,
    )?;
    Ok(())
}

fn valid_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 60
        && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if !ok {
        bail!("nombre inválido '{name}': usa solo minúsculas, dígitos y guion bajo");
    }
    Ok(())
}

/// Crea un interruptor (apagado, salvo que se indique) o actualiza su descripción.
/// `source` dice quién hizo el cambio (`terminal`, `app`, `mcp`) y queda en el historial.
pub fn add(
    registry: &Registry,
    slug: &str,
    name: &str,
    desc: &str,
    on: bool,
    source: &str,
) -> Result<Flags> {
    valid_name(name)?;
    let mut flags = list(registry, slug)?;
    let created = !flags.contains_key(name);
    let flag = flags.entry(name.to_string()).or_insert(Flag { on, desc: String::new(), value: String::new() });
    flag.desc = desc.to_string();
    save(registry, slug, &flags)?;
    let action = if created { "creado" } else { "descripción actualizada" };
    registry.record_event(slug, &format!("interruptor {name}: {action}"), source)?;
    Ok(flags)
}

/// Prende o apaga un interruptor que ya existe.
pub fn set(registry: &Registry, slug: &str, name: &str, on: bool, source: &str) -> Result<Flags> {
    let mut flags = list(registry, slug)?;
    let flag = flags
        .get_mut(name)
        .with_context(|| format!("'{slug}' no tiene un interruptor llamado '{name}'"))?;
    if flag.on != on {
        flag.on = on;
        save(registry, slug, &flags)?;
        let state = if on { "prendido" } else { "apagado" };
        registry.record_event(slug, &format!("interruptor {name}: {state}"), source)?;
    }
    Ok(flags)
}

/// Cambia el texto o valor asociado a un interruptor.
pub fn set_value(
    registry: &Registry,
    slug: &str,
    name: &str,
    value: &str,
    source: &str,
) -> Result<Flags> {
    let mut flags = list(registry, slug)?;
    let flag = flags
        .get_mut(name)
        .with_context(|| format!("'{slug}' no tiene un interruptor llamado '{name}'"))?;
    if flag.value != value {
        flag.value = value.to_string();
        save(registry, slug, &flags)?;
        registry.record_event(slug, &format!("interruptor {name}: texto cambiado"), source)?;
    }
    Ok(flags)
}

/// Conecta una app al almacén: crea una llave de solo lectura y la guarda como la
/// variable `GLOBAL_CONFIG` del proyecto en Vercel. Si se pasa `env_file`, también la
/// escribe ahí para el desarrollo local. La llave nunca se imprime.
pub fn connect(registry: &Registry, slug: &str, env_file: Option<&std::path::Path>) -> Result<()> {
    let project = registry
        .get(slug)?
        .with_context(|| format!("no existe el proyecto '{slug}'"))?;
    let services = crate::inspect::project_services(&project);
    let project_id = services
        .vercel_project_id
        .context("el proyecto no tiene un proyecto de Vercel asociado")?;
    let id = store(registry)?;
    let team = team(registry)?;
    let team = team.as_deref();

    let label = json!({ "label": format!("atalaya-{slug}") });
    let created = expect_ok(
        "crear la llave de lectura",
        vercel("POST", &format!("/v1/edge-config/{id}/token"), team, Some(&label))?,
    )?;
    let token = created["token"]
        .as_str()
        .context("Vercel no devolvió la llave de lectura")?;
    let connection = format!("https://global-config.vercel.com/{id}?token={token}");

    let env = json!({
        "key": "GLOBAL_CONFIG",
        "value": connection,
        "type": "encrypted",
        "target": ["production", "preview", "development"],
    });
    expect_ok(
        "guardar la variable en el proyecto",
        vercel("POST", &format!("/v10/projects/{project_id}/env?upsert=true"), team, Some(&env))?,
    )?;

    if let Some(file) = env_file {
        let previous = std::fs::read_to_string(file).unwrap_or_default();
        let mut lines: Vec<&str> =
            previous.lines().filter(|l| !l.starts_with("GLOBAL_CONFIG=")).collect();
        let line = format!("GLOBAL_CONFIG={connection}");
        lines.push(&line);
        std::fs::write(file, lines.join("\n") + "\n")
            .with_context(|| format!("no se pudo escribir {}", file.display()))?;
    }
    registry.record_event(slug, "app conectada al almacén de interruptores", "terminal")?;
    Ok(())
}

pub fn remove(registry: &Registry, slug: &str, name: &str, source: &str) -> Result<Flags> {
    let mut flags = list(registry, slug)?;
    if flags.remove(name).is_none() {
        bail!("'{slug}' no tiene un interruptor llamado '{name}'");
    }
    save(registry, slug, &flags)?;
    registry.record_event(slug, &format!("interruptor {name}: eliminado"), source)?;
    Ok(flags)
}
