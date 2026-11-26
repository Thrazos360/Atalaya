//! Firebase: estado del proyecto y de sus reglas de seguridad.
//! Entra con una cuenta de servicio de Google (solo lectura), guardada en el
//! Llavero como `firebase`; así funciona igual en el Mac y en Proxmox.

use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Lectura de la configuración del proyecto y, para los respaldos, de los datos de Firestore.
const SCOPES: &str = "https://www.googleapis.com/auth/firebase.readonly \
https://www.googleapis.com/auth/cloud-platform.read-only https://www.googleapis.com/auth/datastore";

/// Guarda la llave de la cuenta de servicio (el archivo JSON que entrega Firebase).
/// Se valida antes de guardar y nunca se imprime.
pub fn connect(file: &Path) -> Result<String> {
    let text = std::fs::read_to_string(file)
        .with_context(|| format!("no se pudo leer {}", file.display()))?;
    let account: Value = serde_json::from_str(&text).context("el archivo no es un JSON válido")?;
    if account["type"] != "service_account" || !account["private_key"].is_string() {
        bail!("el archivo no es una llave de cuenta de servicio de Google");
    }
    let email = account["client_email"].as_str().unwrap_or_default().to_string();
    // En base64, para que quepa como un valor simple del Llavero.
    if !crate::secrets::set("firebase", &STANDARD.encode(account.to_string()))? {
        bail!("no se pudo guardar en el Llavero");
    }
    Ok(email)
}

fn account() -> Option<Value> {
    account_from_saved(&crate::secrets::get("firebase")?)
}

/// La cuenta de servicio tal como se guarda en el Llavero (JSON en base64).
pub(crate) fn account_from_saved(saved: &str) -> Option<Value> {
    serde_json::from_slice(&STANDARD.decode(saved).ok()?).ok()
}

/// Prueba una cuenta de servicio: la canjea por un permiso temporal de Google.
pub(crate) fn test_account(account: &Value) -> Result<String> {
    access_token(account)?;
    Ok(format!(
        "Cuenta de servicio {} válida",
        account["client_email"].as_str().unwrap_or("sin nombre")
    ))
}

pub fn connected() -> bool {
    account().is_some()
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        .http_status_as_error(false)
        .build()
        .into()
}

fn body_json(mut response: ureq::http::Response<ureq::Body>) -> (u16, Value) {
    let parsed = response
        .body_mut()
        .read_to_string()
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    (response.status().as_u16(), parsed)
}

/// Canjea la llave de la cuenta de servicio por un permiso temporal de Google.
fn access_token(account: &Value) -> Result<String> {
    let email = account["client_email"].as_str().context("la llave no tiene client_email")?;
    let key = account["private_key"].as_str().context("la llave no tiene private_key")?;
    let token_uri = account["token_uri"].as_str().unwrap_or("https://oauth2.googleapis.com/token");
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let claims = json!({ "iss": email, "scope": SCOPES, "aud": token_uri, "iat": now, "exp": now + 600 });
    let assertion = jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256),
        &claims,
        &jsonwebtoken::EncodingKey::from_rsa_pem(key.as_bytes()).context("la llave privada no es válida")?,
    )?;
    let form = format!(
        "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Ajwt-bearer&assertion={assertion}"
    );
    let (code, body) = body_json(
        agent()
            .post(token_uri)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .send(form)
            .context("no se pudo conectar con Google")?,
    );
    if code != 200 {
        let detail = body["error_description"].as_str().or(body["error"].as_str()).unwrap_or("sin detalle");
        bail!("Google rechazó la cuenta de servicio (error {code}): {detail}");
    }
    body["access_token"].as_str().map(String::from).context("Google no devolvió un permiso")
}

fn get(url: &str, token: &str) -> Result<(u16, Value)> {
    Ok(body_json(
        agent()
            .get(url)
            .header("Authorization", &format!("Bearer {token}"))
            .call()
            .context("no se pudo conectar con Google")?,
    ))
}

/// Todos los documentos de Firestore, de cualquier colección y nivel, para respaldo.
/// Cada uno conserva su ruta completa (`name`) y sus campos en el formato de la API.
pub fn all_documents(project: &str) -> Result<Vec<Value>> {
    let account = account().context("falta la cuenta de servicio de Firebase")?;
    let token = access_token(&account)?;
    // Sin `collectionId`, la consulta recorre todas las colecciones y subcolecciones.
    let body = json!({ "structuredQuery": { "from": [{ "allDescendants": true }] } });
    let mut response = agent()
        .post(&format!(
            "https://firestore.googleapis.com/v1/projects/{project}/databases/(default)/documents:runQuery"
        ))
        .header("Authorization", &format!("Bearer {token}"))
        .header("Content-Type", "application/json")
        .send(body.to_string())
        .context("no se pudo conectar con Firestore")?;
    let code = response.status().as_u16();
    let text = response
        .body_mut()
        .with_config()
        .limit(1024 * 1024 * 1024)
        .read_to_string()
        .context("no se pudo leer la respuesta de Firestore")?;
    let parsed: Value = serde_json::from_str(&text).context("Firestore devolvió una respuesta inesperada")?;
    if code >= 400 {
        let detail = parsed[0]["error"]["message"].as_str().or(parsed["error"]["message"].as_str()).unwrap_or("sin detalle");
        bail!("Firestore rechazó la lectura (error {code}): {detail}");
    }
    Ok(parsed
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| row.get("document").cloned())
        .collect())
}

/// Sin comentarios ni espacios, para comparar reglas sin que importe el formato.
fn normalize(rules: &str) -> String {
    rules
        .lines()
        .map(|line| line.split("//").next().unwrap_or_default())
        .collect::<String>()
        .split_whitespace()
        .collect()
}

/// Cuenta las reglas `allow …: if true`, que dejan ese acceso abierto a cualquiera:
/// (escrituras públicas, lecturas públicas). Una lectura pública suele ser a propósito
/// (un catálogo); una escritura pública casi nunca lo es.
fn public_access(rules: &str) -> (usize, usize) {
    let text: String = rules
        .lines()
        .map(|line| line.split("//").next().unwrap_or_default())
        .collect::<Vec<_>>()
        .join(" ");
    let (mut writes, mut reads) = (0, 0);
    for statement in text.split([';', '{', '}']) {
        let Some((_, rule)) = statement.split_once("allow ") else { continue };
        let Some((operations, condition)) = rule.split_once(':') else { continue };
        if condition.split_whitespace().collect::<Vec<_>>() != ["if", "true"] {
            continue;
        }
        let writes_here = operations
            .split(',')
            .any(|op| matches!(op.trim(), "write" | "create" | "update" | "delete"));
        if writes_here {
            writes += 1;
        } else {
            reads += 1;
        }
    }
    (writes, reads)
}

/// Estado de un proyecto de Firebase: si está activo y cómo están las reglas de
/// Firestore y Storage (abiertas o no, e iguales o no a las del repo en `folder`).
pub fn status(project: &str, folder: Option<&Path>) -> Result<Value> {
    let account = account().context("falta la cuenta de servicio de Firebase: agrégala en Conexiones")?;
    let token = access_token(&account)?;

    let (code, info) = get(&format!("https://firebase.googleapis.com/v1beta1/projects/{project}"), &token)?;
    if code == 403 || code == 404 {
        bail!("la cuenta de servicio no tiene acceso al proyecto {project} (error {code})");
    }
    let state = info["state"].as_str().unwrap_or("DESCONOCIDO").to_string();

    let (code, releases) = get(
        &format!("https://firebaserules.googleapis.com/v1/projects/{project}/releases"),
        &token,
    )?;
    if code >= 400 {
        let detail = releases["error"]["message"].as_str().unwrap_or("sin detalle");
        bail!("no se pudieron leer las reglas (error {code}): {detail}");
    }

    let mut rules = Vec::new();
    for release in releases["releases"].as_array().into_iter().flatten() {
        let name = release["name"].as_str().unwrap_or_default();
        // "projects/x/releases/cloud.firestore" o ".../firebase.storage/<bucket>".
        let (service, local_file) = if name.contains("/releases/cloud.firestore") {
            ("firestore", "firestore.rules")
        } else if name.contains("/releases/firebase.storage") {
            ("storage", "storage.rules")
        } else {
            continue;
        };
        let Some(ruleset) = release["rulesetName"].as_str() else { continue };
        let (_, body) = get(&format!("https://firebaserules.googleapis.com/v1/{ruleset}"), &token)?;
        let deployed: String = body["source"]["files"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|f| f["content"].as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let local = folder.and_then(|dir| std::fs::read_to_string(dir.join(local_file)).ok());
        let (public_writes, public_reads) = public_access(&deployed);
        rules.push(json!({
            "servicio": service,
            "escrituras_publicas": public_writes,
            "lecturas_publicas": public_reads,
            // `null` cuando no hay archivo local con qué comparar.
            "igual_al_repo": local.map(|l| normalize(&l) == normalize(&deployed)),
            "actualizadas": release["updateTime"],
        }));
    }
    Ok(json!({ "proyecto": project, "estado": state, "reglas": rules }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tells_public_writes_from_public_reads() {
        let rules = "match /catalogo/{id} {
              allow read: if true; // catálogo público
              allow get, list: if true;
              // allow write: if true;
              allow write: if request.auth != null;
            }
            match /buzon/{id} { allow create: if true; }";
        assert_eq!(public_access(rules), (1, 2));
        assert_eq!(public_access("allow read, write: if true;"), (1, 0));
        assert_eq!(normalize("a  {\n b; // nota\n}"), normalize("a { b; }"));
    }
}
