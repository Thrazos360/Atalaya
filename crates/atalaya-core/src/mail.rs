//! Envío de correo por SMTP (Gmail por defecto, con contraseña de aplicación).
//! La dirección vive en los ajustes; la contraseña, en el Llavero (`gmail`); el
//! servidor, en la conexión de correo.

use crate::Registry;
use anyhow::{Context, Result};
use lettre::message::header::ContentType;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{Message, SmtpTransport, Transport};

pub const ADDRESS: &str = "email_address";
pub const TO: &str = "email_to";
pub const REPORT_HOUR: &str = "report_hour";
const REPORT_LAST_DATE: &str = "report_last_date";

/// Hay dirección y contraseña guardadas.
pub fn configured(registry: &Registry) -> Result<bool> {
    Ok(registry.setting(ADDRESS)?.is_some() && crate::secrets::get("gmail").is_some())
}

pub fn send(registry: &Registry, subject: &str, body: &str) -> Result<()> {
    let from = registry
        .setting(ADDRESS)?
        .context("falta la dirección de correo: configúrala en Conexiones → Avisos → Correo")?;
    let to = registry.setting(TO)?.unwrap_or_else(|| from.clone());
    let password =
        crate::secrets::get("gmail").context("falta la contraseña de aplicación de Gmail")?;

    let email = Message::builder()
        .from(format!("Atalaya <{from}>").parse().context("dirección de origen inválida")?)
        .to(to.parse().context("dirección de destino inválida")?)
        .subject(subject)
        .header(ContentType::TEXT_PLAIN)
        .body(body.to_string())?;
    // El servidor sale de la conexión de correo; sin ella, Gmail como siempre.
    let connection = crate::connections::default_for(registry, "correo")?;
    let host = connection
        .as_ref()
        .and_then(|c| c.config_str("smtp_host"))
        .unwrap_or_else(|| "smtp.gmail.com".to_string());
    let port: u16 = connection
        .as_ref()
        .and_then(|c| c.config_str("smtp_port"))
        .and_then(|p| p.parse().ok())
        .unwrap_or(465);
    // 465 es TLS directo; 587 (y los demás) negocian TLS con STARTTLS.
    let builder = if port == 465 { SmtpTransport::relay(&host)? } else { SmtpTransport::starttls_relay(&host)? };
    builder
        .port(port)
        .credentials(Credentials::new(from, password))
        .build()
        .send(&email)
        .with_context(|| format!("{host} no aceptó el envío"))?;
    Ok(())
}

/// Envía el resumen diario la primera vez que se llama después de la hora
/// configurada (por defecto las 8, hora local). Devuelve `true` si lo envió.
pub fn daily_report(registry: &Registry) -> Result<bool> {
    if !configured(registry)? {
        return Ok(false);
    }
    let (today, hour) = registry.local_now()?;
    let wanted: u32 = registry
        .setting(REPORT_HOUR)?
        .and_then(|h| h.parse().ok())
        .unwrap_or(8);
    if hour < wanted || registry.setting(REPORT_LAST_DATE)?.as_deref() == Some(today.as_str()) {
        return Ok(false);
    }
    send(
        registry,
        &format!("Atalaya: resumen del {today}"),
        &crate::checks::report(registry)?,
    )?;
    registry.set_setting(REPORT_LAST_DATE, &today)?;
    Ok(true)
}
