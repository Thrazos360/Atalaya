//! Llaves de los servicios externos. Nunca se guardan en la base de datos ni en archivos:
//! viven en el Llavero de macOS, o en variables de entorno (para Proxmox).

use std::process::{Command, Stdio};

/// Servicios para los que Atalaya sabe usar una llave.
pub const SERVICES: [&str; 7] = ["vercel", "supabase", "gmail", "github", "firebase", "claude", "telegram"];

/// Nombre del grupo de llaves en el Llavero. `ATALAYA_KEYCHAIN_SERVICE` lo cambia, para
/// probar una copia de Atalaya con un Llavero vacío sin tocar las llaves de verdad.
fn keychain_service() -> String {
    std::env::var("ATALAYA_KEYCHAIN_SERVICE")
        .ok()
        .filter(|s| !s.trim().is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)))
        .unwrap_or_else(|| "Atalaya".to_string())
}

/// Busca primero `ATALAYA_<SERVICIO>_TOKEN` y después el Llavero.
pub fn get(service: &str) -> Option<String> {
    let var = format!(
        "ATALAYA_{}_TOKEN",
        service.to_uppercase().replace(|c: char| !c.is_ascii_alphanumeric(), "_")
    );
    if let Ok(value) = std::env::var(var) {
        if !value.is_empty() {
            return Some(value);
        }
    }
    get_keychain(service)
}

fn get_keychain(service: &str) -> Option<String> {
    let out = Command::new("security")
        .args(["find-generic-password", "-s", &keychain_service(), "-a", service, "-w"])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let value = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !value.is_empty()).then_some(value)
}

/// Guarda (o reemplaza) la llave en el Llavero. La llave viaja por la entrada
/// estándar de `security`, no como argumento, para que no aparezca en la lista de procesos.
pub fn set(service: &str, value: &str) -> std::io::Result<bool> {
    use std::io::{Error, ErrorKind, Write};
    if value.is_empty() || value.chars().any(|c| c.is_whitespace() || c == '"' || c == '\\') {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            "la llave está vacía o tiene espacios o comillas",
        ));
    }
    let mut child = Command::new("security")
        .arg("-i")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    if service.is_empty() || !service.chars().all(|c| c.is_ascii_alphanumeric() || "-_.:".contains(c)) {
        return Err(Error::new(ErrorKind::InvalidInput, "nombre de llave inválido"));
    }
    let command = format!(
        "add-generic-password -U -s {} -a {service} -w \"{value}\"\n",
        keychain_service()
    );
    child.stdin.take().expect("stdin").write_all(command.as_bytes())?;
    child.wait()?;
    // `security -i` no informa el resultado de cada orden: se comprueba leyendo.
    Ok(get_keychain(service).as_deref() == Some(value))
}

pub fn remove(service: &str) -> std::io::Result<bool> {
    Ok(Command::new("security")
        .args(["delete-generic-password", "-s", &keychain_service(), "-a", service])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?
        .success())
}

/// Guarda cualquier texto (una URL con símbolos, un JSON con espacios): va en base64
/// con la marca `b64:`, porque el Llavero se escribe con una orden de una línea.
pub fn set_text(account: &str, value: &str) -> std::io::Result<bool> {
    use base64::Engine;
    if value.is_empty() {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "la llave está vacía"));
    }
    set(account, &format!("b64:{}", base64::engine::general_purpose::STANDARD.encode(value)))
}

/// Lee lo guardado con `set_text`, o el valor tal cual si se guardó con `set`.
pub fn get_text(account: &str) -> Option<String> {
    use base64::Engine;
    let raw = get(account)?;
    match raw.strip_prefix("b64:") {
        Some(encoded) => {
            let bytes = base64::engine::general_purpose::STANDARD.decode(encoded).ok()?;
            String::from_utf8(bytes).ok()
        }
        None => Some(raw),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Escribe en el Llavero real, así que solo corre a pedido:
    /// `cargo test -p atalaya-core -- --ignored`.
    #[test]
    #[ignore]
    fn keychain_round_trip() {
        let account = "prueba-atalaya";
        assert!(set(account, "abc_123-XYZ").unwrap());
        assert!(set(account, "otra-456").unwrap(), "debe reemplazar la anterior");
        assert_eq!(get_keychain(account).as_deref(), Some("otra-456"));
        assert!(remove(account).unwrap());
        assert_eq!(get_keychain(account), None);
        assert!(set(account, "con espacio").is_err());
    }
}
