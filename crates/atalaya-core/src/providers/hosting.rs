//! Hosting: Vercel, Netlify y servidores propios.

use super::{Outcome, expect_2xx, http};
use crate::checks::{FAIL, OK, WARN};
use crate::connections::{Connection, Link};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::process::Command;
use std::time::Duration;

fn bearer(c: &Connection) -> Result<Vec<(&'static str, String)>> {
    let token = c.secret().with_context(|| format!("la conexión «{}» no tiene llave guardada", c.name))?;
    Ok(vec![("Authorization", format!("Bearer {token}"))])
}

/// Prueba la llave de Vercel y dice a qué equipos da acceso.
pub fn vercel_test(c: &Connection) -> Result<String> {
    let auth = bearer(c)?;
    let user = expect_2xx("Vercel", http("GET", "https://api.vercel.com/v2/user", &auth, None, 30)?)?;
    let name = user["user"]["username"].as_str().unwrap_or("sin nombre");
    let teams = expect_2xx("Vercel", http("GET", "https://api.vercel.com/v2/teams?limit=20", &auth, None, 30)?)
        .unwrap_or(Value::Null);
    let teams: Vec<String> = teams["teams"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| Some(format!("{} ({})", t["name"].as_str()?, t["id"].as_str()?)))
        .collect();
    Ok(if teams.is_empty() {
        format!("Conectado como {name}")
    } else {
        format!("Conectado como {name} · equipos: {}", teams.join(", "))
    })
}

pub fn netlify_test(c: &Connection) -> Result<String> {
    let auth = bearer(c)?;
    let user = expect_2xx("Netlify", http("GET", "https://api.netlify.com/api/v1/user", &auth, None, 30)?)?;
    let sites = expect_2xx("Netlify", http("GET", "https://api.netlify.com/api/v1/sites?per_page=100", &auth, None, 30)?)?;
    Ok(format!(
        "Conectado como {} · {} sitios",
        user["email"].as_str().or(user["full_name"].as_str()).unwrap_or("sin nombre"),
        sites.as_array().map_or(0, Vec::len)
    ))
}

/// Último despliegue de producción de un sitio de Netlify.
pub fn netlify_check(c: &Connection, link: &Link) -> Option<Outcome> {
    let site = link.config_str("site_id")?;
    let result = (|| -> Result<Outcome> {
        let deploys = expect_2xx(
            "Netlify",
            http(
                "GET",
                &format!("https://api.netlify.com/api/v1/sites/{site}/deploys?per_page=1"),
                &bearer(c)?,
                None,
                30,
            )?,
        )?;
        let last = deploys.get(0).cloned().unwrap_or(Value::Null);
        let state = last["state"].as_str().unwrap_or("");
        let title = last["title"].as_str().and_then(|t| t.lines().next()).unwrap_or_default();
        let suffix = if title.is_empty() { String::new() } else { format!(": {title}") };
        let details = json!({ "estado": state, "url": last["deploy_ssl_url"], "creado": last["created_at"] });
        Ok(match state {
            "ready" => (OK, format!("Último despliegue correcto{suffix}"), details),
            "building" | "enqueued" | "processing" | "uploading" | "preparing" => (OK, format!("Desplegando{suffix}"), details),
            "error" => (FAIL, format!("El último despliegue falló{suffix}"), details),
            "" => (WARN, "Sin despliegues".into(), details),
            other => (WARN, format!("Despliegue en estado {other}"), details),
        })
    })();
    Some(result.unwrap_or_else(|e| (WARN, "No se pudo consultar Netlify".into(), json!({ "error": format!("{e:#}") }))))
}

/// Ejecuta una orden por SSH sin pedir nada en pantalla: usa las llaves y el agente SSH
/// del sistema. Atalaya no lee ni guarda llaves SSH.
fn ssh(c: &Connection, script: &str) -> Result<String> {
    let host = c.config_str("ssh_host").context("la conexión no tiene host SSH")?;
    if host.starts_with('-') || host.contains(char::is_whitespace) {
        bail!("host SSH inválido");
    }
    let user = c.config_str("ssh_user").unwrap_or_else(|| "root".into());
    if user.starts_with('-') || user.contains(char::is_whitespace) {
        bail!("usuario SSH inválido");
    }
    let port = c.config_str("ssh_port").unwrap_or_else(|| "22".into());
    if port.parse::<u16>().is_err() {
        bail!("puerto SSH inválido");
    }
    let out = Command::new("ssh")
        .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=10", "-o", "StrictHostKeyChecking=accept-new", "-p", &port])
        .arg(format!("{user}@{host}"))
        .arg(script)
        .output()
        .context("no se pudo ejecutar ssh")?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        bail!("SSH falló: {}", if err.is_empty() { "sin detalle".into() } else { err });
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Lo que se mide por SSH: carga y uso del disco raíz.
#[derive(Debug, PartialEq)]
struct Vitals {
    load: Option<f64>,
    disk_percent: Option<u64>,
    uptime: String,
}

const VITALS_SCRIPT: &str = "uptime; df -P / | tail -1";

fn parse_vitals(text: &str) -> Vitals {
    let mut lines = text.lines();
    let uptime = lines.next().unwrap_or_default().trim().to_string();
    let load = uptime
        .rsplit_once("load average")
        .or_else(|| uptime.rsplit_once("load averages"))
        .and_then(|(_, rest)| rest.trim_start_matches(['s', ':', ' ']).split([',', ' ']).find(|s| !s.is_empty())?.parse().ok());
    let disk_percent = lines
        .next()
        .and_then(|l| l.split_whitespace().find(|w| w.ends_with('%'))?.trim_end_matches('%').parse().ok());
    Vitals { load, disk_percent, uptime }
}

fn health(url: &str) -> Outcome {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(20)))
        .http_status_as_error(false)
        .user_agent("Atalaya")
        .build()
        .into();
    let start = std::time::Instant::now();
    match agent.get(url).call() {
        Ok(r) => {
            let code = r.status().as_u16();
            let ms = start.elapsed().as_millis() as u64;
            let details = json!({ "url": url, "codigo": code, "ms": ms });
            if code >= 400 {
                (FAIL, format!("La URL de salud responde {code}"), details)
            } else {
                (OK, format!("Salud en orden ({ms} ms)"), details)
            }
        }
        Err(e) => (FAIL, "La URL de salud no responde".into(), json!({ "url": url, "error": e.to_string() })),
    }
}

pub fn server_test(c: &Connection) -> Result<String> {
    let mut parts = Vec::new();
    if let Some(url) = c.config_str("health_url") {
        let (status, summary, _) = health(&url);
        if status == FAIL {
            bail!("{summary}");
        }
        parts.push(summary);
    }
    if c.config_str("ssh_host").is_some() {
        let vitals = parse_vitals(&ssh(c, VITALS_SCRIPT)?);
        parts.push(format!(
            "SSH en orden · carga {} · disco {}",
            vitals.load.map_or("—".into(), |l| format!("{l:.2}")),
            vitals.disk_percent.map_or("—".into(), |d| format!("{d} %"))
        ));
    }
    if parts.is_empty() {
        bail!("agrega una URL de salud o un host SSH para poder revisar este servidor");
    }
    Ok(parts.join(" · "))
}

/// Revisión de un servidor propio: su URL de salud y, con SSH, la carga y el disco.
pub fn server_check(c: &Connection) -> Option<Outcome> {
    let url = c.config_str("health_url");
    let has_ssh = c.config_str("ssh_host").is_some();
    if url.is_none() && !has_ssh {
        return None;
    }
    let mut status = OK;
    let mut parts = Vec::new();
    let mut details = json!({});
    if let Some(url) = url {
        let (s, summary, d) = health(&url);
        if s == FAIL {
            status = FAIL;
        }
        parts.push(summary);
        details["salud"] = d;
    }
    if has_ssh {
        match ssh(c, VITALS_SCRIPT) {
            Ok(text) => {
                let v = parse_vitals(&text);
                if let Some(d) = v.disk_percent {
                    if d >= 95 {
                        status = FAIL;
                        parts.push(format!("disco al {d} %"));
                    } else if d >= 85 {
                        if status == OK {
                            status = WARN;
                        }
                        parts.push(format!("disco al {d} %"));
                    } else {
                        parts.push(format!("disco {d} %"));
                    }
                }
                if let Some(l) = v.load {
                    parts.push(format!("carga {l:.2}"));
                }
                details["ssh"] = json!({ "carga": v.load, "disco": v.disk_percent, "uptime": v.uptime });
            }
            Err(e) => {
                if status == OK {
                    status = WARN;
                }
                parts.push("sin acceso SSH".into());
                details["ssh"] = json!({ "error": format!("{e:#}") });
            }
        }
    }
    Some((status, parts.join(" · "), details))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_linux_and_mac_uptime() {
        let linux = " 10:00:01 up 12 days,  3:04,  1 user,  load average: 0.52, 0.40, 0.31\n/dev/sda1  51475068 41180000 7672472  85% /\n";
        let v = parse_vitals(linux);
        assert_eq!(v.load, Some(0.52));
        assert_eq!(v.disk_percent, Some(85));
        let mac = "10:00  up 3 days, 2:01, 2 users, load averages: 1.95 2.10 2.20\n/dev/disk3s1s1 1000 500 500 50% /\n";
        let v = parse_vitals(mac);
        assert_eq!(v.load, Some(1.95));
        assert_eq!(v.disk_percent, Some(50));
        assert_eq!(parse_vitals("").load, None);
    }

    #[test]
    fn server_without_url_or_ssh_is_not_checked() {
        let c = Connection {
            id: "s".into(),
            kind: "hosting".into(),
            provider: "servidor".into(),
            name: "Casa".into(),
            config: json!({}),
            secret_account: None,
            created_at: String::new(),
        };
        assert!(server_check(&c).is_none());
        assert!(server_test(&c).is_err());
    }
}
