//! Comandos guardados: recetas que se llaman desde el chat con `/nombre` y sirven con
//! cualquier IA. La plantilla recibe lo que escribas después del nombre en `{texto}` (o al
//! final, si no lo usa).

use crate::Registry;
use anyhow::{Result, bail};
use rusqlite::params;
use serde::Serialize;

pub fn init(registry: &Registry) -> Result<()> {
    registry.conn().execute_batch(
        "CREATE TABLE IF NOT EXISTS commands (
            name TEXT PRIMARY KEY,
            description TEXT NOT NULL DEFAULT '',
            template TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );",
    )?;
    Ok(())
}

pub const PLACEHOLDER: &str = "{texto}";

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SavedCommand {
    pub name: String,
    pub description: String,
    pub template: String,
}

/// Unos de ejemplo, para partir.
pub const EXAMPLES: &[(&str, &str, &str)] = &[
    (
        "revisar-seguridad",
        "Busca llaves en el código, dependencias con fallas y entradas sin validar",
        "Revisa la seguridad de {texto}: llaves o secretos en el código, dependencias con fallas conocidas, permisos de más y entradas sin validar. Ordena lo que encuentres por gravedad y propón el arreglo de cada punto. No cambies nada todavía.",
    ),
    (
        "subir-dependencias",
        "Sube las dependencias que no rompen nada",
        "Revisa qué dependencias de {texto} tienen versiones nuevas. Sube las que no rompen nada (corre las pruebas y el build para comprobarlo) y dime cuáles necesitan cambios en el código.",
    ),
    (
        "explicar",
        "Explica una parte del código en palabras simples",
        "Explícame {texto} en palabras simples: qué hace, cómo se conecta con el resto y qué hay que cuidar al cambiarlo.",
    ),
    (
        "resumen-semana",
        "Lo que pasó esta semana en un proyecto",
        "Resume lo que pasó esta semana en {texto}: commits, mejoras, decisiones, alertas y consumo de IA. Lo importante primero, en pocas líneas.",
    ),
];

fn clean_name(name: &str) -> String {
    let lower = name.trim().trim_start_matches('/').to_lowercase();
    let mut out = String::new();
    for c in lower.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_matches('-').chars().take(40).collect()
}

pub fn list(registry: &Registry) -> Result<Vec<SavedCommand>> {
    let mut stmt = registry.conn().prepare("SELECT name, description, template FROM commands ORDER BY name")?;
    let rows = stmt.query_map([], |r| Ok(SavedCommand { name: r.get(0)?, description: r.get(1)?, template: r.get(2)? }))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Guarda (o reemplaza) un comando.
pub fn save(registry: &Registry, name: &str, description: &str, template: &str) -> Result<SavedCommand> {
    let name = clean_name(name);
    if name.is_empty() {
        bail!("ponle un nombre al comando");
    }
    let template = template.trim();
    if template.is_empty() {
        bail!("escribe qué le pide el comando a la IA");
    }
    registry.conn().execute(
        "INSERT INTO commands (name, description, template) VALUES (?1, ?2, ?3)
         ON CONFLICT(name) DO UPDATE SET description = ?2, template = ?3",
        params![name, description.trim(), template],
    )?;
    Ok(SavedCommand { name, description: description.trim().to_string(), template: template.to_string() })
}

/// Agrega los de ejemplo que falten. Devuelve cuántos agregó.
pub fn add_examples(registry: &Registry) -> Result<usize> {
    let mut added = 0;
    for (name, description, template) in EXAMPLES {
        added += registry.conn().execute(
            "INSERT OR IGNORE INTO commands (name, description, template) VALUES (?1, ?2, ?3)",
            params![name, description, template],
        )?;
    }
    Ok(added)
}

pub fn remove(registry: &Registry, name: &str) -> Result<()> {
    if registry.conn().execute("DELETE FROM commands WHERE name = ?1", [name])? == 0 {
        bail!("no hay un comando «{name}»");
    }
    Ok(())
}

/// Arma el mensaje de un comando con lo que se escribió después de su nombre.
pub fn fill(template: &str, text: &str) -> String {
    let text = text.trim();
    if template.contains(PLACEHOLDER) {
        template.replace(PLACEHOLDER, if text.is_empty() { "lo que estamos viendo" } else { text })
    } else if text.is_empty() {
        template.to_string()
    } else {
        format!("{template}\n\n{text}")
    }
}

/// Si el mensaje empieza con `/nombre` de un comando guardado, el mensaje completo.
pub fn expand(registry: &Registry, message: &str) -> Result<Option<String>> {
    let Some(rest) = message.trim_start().strip_prefix('/') else { return Ok(None) };
    let (name, text) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    Ok(list(registry)?.into_iter().find(|c| c.name == name).map(|c| fill(&c.template, text)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_and_expanded() {
        let dir = std::env::temp_dir().join(format!("atalaya-comandos-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let r = Registry::open(&dir.join("a.db")).unwrap();
        assert_eq!(add_examples(&r).unwrap(), EXAMPLES.len());
        assert_eq!(add_examples(&r).unwrap(), 0);
        save(&r, "/Mi Receta", "", "Hazlo así.").unwrap();
        assert_eq!(expand(&r, "/mi-receta con cuidado").unwrap().unwrap(), "Hazlo así.\n\ncon cuidado");
        let e = expand(&r, "/explicar el login").unwrap().unwrap();
        assert!(e.starts_with("Explícame el login en palabras simples"), "{e}");
        assert!(expand(&r, "/explicar").unwrap().unwrap().contains("lo que estamos viendo"));
        assert_eq!(expand(&r, "/no-existe x").unwrap(), None);
        assert_eq!(expand(&r, "hola").unwrap(), None);
        remove(&r, "mi-receta").unwrap();
        assert!(remove(&r, "mi-receta").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
