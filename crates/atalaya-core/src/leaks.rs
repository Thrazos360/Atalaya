//! Revisión de secretos antes de subir: busca llaves, tokens y archivos sensibles en lo
//! que agrega un cambio. El constructor la corre antes de abrir cada pull request; si
//! encuentra algo, la rama no sale del Mac y la mejora lo explica.
//!
//! Nunca muestra el secreto: solo dónde está, qué parece y sus primeros caracteres.
//! Una línea con `atalaya:permitir` se salta (para ejemplos y pruebas a propósito).

use anyhow::{Context, Result, bail};
use base64::Engine;
use regex::Regex;
use serde::Serialize;
use std::path::Path;
use std::process::Command;
use std::sync::OnceLock;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Finding {
    pub file: String,
    /// Línea en el archivo nuevo; 0 si el hallazgo es el archivo entero.
    pub line: usize,
    /// Qué parece: «llave de Anthropic», «archivo .env»…
    pub kind: String,
    /// Los primeros caracteres y el largo, nunca el secreto completo.
    pub preview: String,
}

impl Finding {
    pub fn describe(&self) -> String {
        if self.line == 0 {
            format!("{}: {}", self.file, self.kind)
        } else {
            format!("{}:{}: {} ({})", self.file, self.line, self.kind, self.preview)
        }
    }
}

const ALLOW: &str = "atalaya:permitir";

fn patterns() -> &'static [(Regex, &'static str)] {
    static PATTERNS: OnceLock<Vec<(Regex, &'static str)>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        [
            (r"-----BEGIN (?:RSA |EC |DSA |OPENSSH |PGP |ENCRYPTED )?PRIVATE KEY", "llave privada"),
            (r"\bsk-ant-[A-Za-z0-9_\-]{20,}", "llave de Anthropic"),
            (r"\bsk-(?:proj-|svcacct-)?[A-Za-z0-9_\-]{32,}", "llave de OpenAI u otra compatible"),
            (r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b", "llave de acceso de AWS"),
            (r"\bgh[pousr]_[A-Za-z0-9]{36,}\b", "token de GitHub"),
            (r"\bgithub_pat_[A-Za-z0-9_]{40,}\b", "token de GitHub"),
            (r"\bglpat-[A-Za-z0-9_\-]{20,}\b", "token de GitLab"),
            (r"\bxox[baprs]-[A-Za-z0-9\-]{10,}", "token de Slack"),
            (r"\b(?:sk|rk)_live_[A-Za-z0-9]{16,}\b", "llave de Stripe en producción"),
            (r"\bAIza[0-9A-Za-z_\-]{35}\b", "llave de Google"),
            (r"\bsbp_[a-f0-9]{40}\b", "token de acceso de Supabase"),
            (r"\bsb_secret_[A-Za-z0-9_\-]{20,}", "llave secreta de Supabase"),
            (r"\b[0-9]{8,10}:AA[A-Za-z0-9_\-]{33}\b", "token de bot de Telegram"),
            (r"\b(?:postgres(?:ql)?|mysql|mongodb(?:\+srv)?)://[^\s:/@]+:[^\s@/]{4,}@", "dirección de base con contraseña"),
        ]
        .into_iter()
        .map(|(re, kind)| (Regex::new(re).expect("patrón válido"), kind))
        .collect()
    })
}

/// `password = "…"`, `api_key: '…'` y parecidos, con un valor que no sea de ejemplo.
fn assignment() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r#"(?i)\b(?:password|passwd|contraseña|secret|client_secret|api_?key|access_?token|auth_?token|private_?key)\b["']?\s*[:=]\s*["']([^"'\s]{12,})["']"#,
        )
        .expect("patrón válido")
    })
}

fn jwt() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\beyJ[A-Za-z0-9_\-]{8,}\.(eyJ[A-Za-z0-9_\-]{8,})\.[A-Za-z0-9_\-]{8,}").expect("patrón válido"))
}

/// Valores que se ven en ejemplos y plantillas, no secretos de verdad.
fn placeholder(value: &str) -> bool {
    let lower = value.to_lowercase();
    ["xxx", "your", "tu_", "tu-", "example", "ejemplo", "changeme", "placeholder", "dummy", "fake", "falso", "test", "prueba", "<", "${", "{{", "process.env", "env("]
        .iter()
        .any(|p| lower.contains(p))
        || value.chars().all(|c| c == value.chars().next().unwrap_or('x'))
}

/// Los primeros caracteres y el largo: suficiente para ubicarlo, inútil para usarlo.
fn preview(secret: &str) -> String {
    let start: String = secret.chars().take(6).collect();
    format!("{start}… de {} caracteres", secret.chars().count())
}

/// Un JWT con rol `service_role` (la llave maestra de Supabase). La `anon` es pública a propósito.
fn service_role(payload: &str) -> bool {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .ok()
        .and_then(|raw| serde_json::from_slice::<serde_json::Value>(&raw).ok())
        .is_some_and(|claims| claims["role"].as_str() == Some("service_role"))
}

/// Lo que parece secreto en una línea agregada.
pub fn scan_line(file: &str, line: usize, text: &str) -> Vec<Finding> {
    if text.contains(ALLOW) {
        return Vec::new();
    }
    let mut found = Vec::new();
    let push = |found: &mut Vec<Finding>, kind: &str, secret: &str| {
        found.push(Finding { file: file.into(), line, kind: kind.into(), preview: preview(secret) });
    };
    // Un mismo secreto cuenta una vez, con el patrón más preciso (van de más a menos).
    let mut spans: Vec<(usize, usize)> = Vec::new();
    for (re, kind) in patterns() {
        if let Some(m) = re.find(text) {
            let overlaps = spans.iter().any(|(a, b)| m.start() < *b && *a < m.end());
            if !overlaps && !placeholder(m.as_str()) {
                spans.push((m.start(), m.end()));
                push(&mut found, kind, m.as_str());
            }
        }
    }
    if let Some(c) = jwt().captures(text) {
        if service_role(&c[1]) {
            push(&mut found, "llave service_role de Supabase", &c[0]);
        }
    }
    if found.is_empty() {
        if let Some(c) = assignment().captures(text) {
            let value = &c[1];
            if !placeholder(value) {
                push(&mut found, "contraseña o llave escrita en el código", value);
            }
        }
    }
    found
}

/// Archivos que no deberían subirse nunca, por su nombre.
pub fn sensitive_file(path: &str) -> Option<&'static str> {
    let name = path.rsplit('/').next().unwrap_or(path).to_lowercase();
    let examples = [".example", ".sample", ".template", ".dist", ".ejemplo"];
    if name == ".env" || (name.starts_with(".env.") && !examples.iter().any(|e| name.ends_with(e))) {
        return Some("archivo .env con variables de entorno");
    }
    if ["id_rsa", "id_dsa", "id_ecdsa", "id_ed25519"].contains(&name.as_str()) {
        return Some("llave SSH privada");
    }
    if [".pem", ".p12", ".pfx", ".key", ".keystore", ".jks", ".p8"].iter().any(|e| name.ends_with(e)) {
        return Some("archivo de llave o certificado privado");
    }
    if name == "credentials.json" || name == ".npmrc" || name == ".pypirc" || name == ".netrc" {
        return Some("archivo de credenciales");
    }
    if name.ends_with(".json") && (name.contains("service-account") || name.contains("serviceaccount") || name.contains("firebase-adminsdk")) {
        return Some("cuenta de servicio de Google o Firebase");
    }
    None
}

/// Revisa un diff unificado (`git diff -U0`): solo las líneas agregadas cuentan.
pub fn scan_diff(diff: &str) -> Vec<Finding> {
    let mut found = Vec::new();
    let mut file = String::new();
    let mut line = 0usize;
    for raw in diff.lines() {
        if let Some(path) = raw.strip_prefix("+++ ") {
            file = path.strip_prefix("b/").unwrap_or(path).to_string();
            continue;
        }
        if raw.starts_with("--- ") || raw.starts_with("diff --git") {
            continue;
        }
        if let Some(header) = raw.strip_prefix("@@ ") {
            // @@ -a,b +c,d @@: la primera línea agregada es la c.
            line = header
                .split_whitespace()
                .find_map(|p| p.strip_prefix('+'))
                .and_then(|p| p.split(',').next())
                .and_then(|n| n.parse().ok())
                .unwrap_or(1);
            continue;
        }
        if file == "/dev/null" {
            continue;
        }
        if let Some(added) = raw.strip_prefix('+') {
            found.extend(scan_line(&file, line, added));
            line += 1;
        }
    }
    found
}

fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git").args(args).current_dir(dir).output().context("no se pudo correr git")?;
    if !out.status.success() {
        bail!("git {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Lo que agrega `dir` desde `base`: los commits y, si `worktree`, también lo que aún no
/// se confirma (incluidos los archivos nuevos sin seguimiento).
pub fn scan_repo(dir: &Path, base: &str, worktree: bool) -> Result<Vec<Finding>> {
    let range = if worktree { base.to_string() } else { format!("{base}..HEAD") };
    let mut found = Vec::new();
    let names = git(dir, &["diff", "--name-only", "--diff-filter=AR", &range])?;
    let mut added: Vec<String> = names.lines().map(String::from).collect();
    if worktree {
        added.extend(git(dir, &["ls-files", "--others", "--exclude-standard"])?.lines().map(String::from));
    }
    for path in &added {
        if let Some(kind) = sensitive_file(path) {
            found.push(Finding { file: path.clone(), line: 0, kind: kind.into(), preview: String::new() });
        }
    }
    found.extend(scan_diff(&git(dir, &["diff", "-U0", "--no-color", "--no-ext-diff", &range])?));
    if worktree {
        // Los archivos nuevos sin seguimiento no salen en el diff: se leen enteros.
        for path in git(dir, &["ls-files", "--others", "--exclude-standard"])?.lines() {
            let Ok(text) = std::fs::read_to_string(dir.join(path)) else { continue };
            for (i, line) in text.lines().enumerate() {
                found.extend(scan_line(path, i + 1, line));
            }
        }
    }
    Ok(found)
}

/// El texto para la mejora o la terminal.
pub fn report(found: &[Finding]) -> String {
    let mut lines: Vec<String> = found.iter().take(20).map(|f| format!("- {}", f.describe())).collect();
    if found.len() > 20 {
        lines.push(format!("- y {} más", found.len() - 20));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_real_looking_keys_and_skips_examples() {
        // Armadas por partes para que este mismo archivo no parezca tener secretos.
        let anthropic = format!("{}{}", "sk-ant-", "api03-AbCdEfGhIjKlMnOpQrStUvWxYz0123456789");
        let found = scan_line("src/config.ts", 3, &format!("const key = \"{anthropic}\";"));
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].kind, "llave de Anthropic");
        assert!(!found[0].preview.contains("AbCdEf"), "no muestra el secreto");

        let github = format!("{}{}", "ghp_", "a1B2c3D4e5F6g7H8i9J0k1L2m3N4o5P6q7R8");
        assert_eq!(scan_line("a", 1, &format!("token: {github}"))[0].kind, "token de GitHub");
        let aws = format!("{}{}", "AKIA", "QWERTYUIOPASDFGH");
        assert_eq!(scan_line("a", 1, &aws)[0].kind, "llave de acceso de AWS");
        assert_eq!(
            scan_line("a", 1, "DATABASE_URL=postgres://app:s3cr3tPass@db.host:5432/app")[0].kind, // atalaya:permitir (dato de prueba inventado)
            "dirección de base con contraseña"
        );
        assert_eq!(scan_line("a", 1, r#"password = "Tr0ub4dor&3xyz""#)[0].kind, "contraseña o llave escrita en el código"); // atalaya:permitir (dato de prueba inventado)

        // Lo de ejemplo, lo que lee del entorno y lo marcado a propósito no cuenta.
        assert!(scan_line("a", 1, r#"password = "your-password-here""#).is_empty());
        assert!(scan_line("a", 1, "const key = process.env.ANTHROPIC_API_KEY;").is_empty());
        assert!(scan_line("a", 1, r#"api_key: "xxxxxxxxxxxxxxxx""#).is_empty());
        assert!(scan_line("a", 1, &format!("{github} // atalaya:permitir")).is_empty());
    }

    #[test]
    fn a_supabase_service_role_jwt_counts_but_the_anon_one_does_not() {
        let encode = |json: &str| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json);
        let header = encode(r#"{"alg":"HS256","typ":"JWT"}"#);
        let token = |role: &str| format!("{header}.{}.firmaFirmaFirma123", encode(&format!(r#"{{"iss":"supabase","role":"{role}"}}"#)));
        assert_eq!(scan_line("a", 1, &token("service_role"))[0].kind, "llave service_role de Supabase");
        assert!(scan_line("a", 1, &token("anon")).is_empty());
    }

    #[test]
    fn reads_only_added_lines_with_their_numbers() {
        let github = format!("{}{}", "ghp_", "a1B2c3D4e5F6g7H8i9J0k1L2m3N4o5P6q7R8");
        let diff = format!(
            "diff --git a/app.js b/app.js\n--- a/app.js\n+++ b/app.js\n@@ -10,2 +10,3 @@\n-const old = \"{github}\";\n+const a = 1;\n+const t = \"{github}\";\n"
        );
        let found = scan_diff(&diff);
        assert_eq!(found.len(), 1, "la línea quitada no cuenta: {found:?}");
        assert_eq!((found[0].file.as_str(), found[0].line), ("app.js", 11));
    }

    #[test]
    fn sensitive_files_by_name() {
        assert!(sensitive_file(".env").is_some());
        assert!(sensitive_file("web/.env.production").is_some());
        assert!(sensitive_file(".env.example").is_none());
        assert!(sensitive_file("keys/id_ed25519").is_some());
        assert!(sensitive_file("certs/server.pem").is_some());
        assert!(sensitive_file("app-firebase-adminsdk-x1.json").is_some());
        assert!(sensitive_file("src/index.ts").is_none());
    }

    #[test]
    fn scans_a_real_repo_with_commits_and_new_files() {
        let dir = std::env::temp_dir().join(format!("atalaya-leaks-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let sh = |cmd: &str| {
            let ok = Command::new("sh").arg("-c").arg(cmd).current_dir(&dir).status().unwrap().success();
            assert!(ok, "{cmd}");
        };
        let id = "-c user.name=Prueba -c user.email=prueba@ejemplo.invalid";
        sh(&format!("git init -q -b main && echo hola > a.txt && git add a.txt && git {id} commit -q -m uno"));
        let base = git(&dir, &["rev-parse", "HEAD"]).unwrap().trim().to_string();
        let github = format!("{}{}", "ghp_", "a1B2c3D4e5F6g7H8i9J0k1L2m3N4o5P6q7R8");
        std::fs::write(dir.join("b.js"), format!("const t = \"{github}\";\n")).unwrap();
        sh(&format!("git add b.js && git {id} commit -q -m dos"));
        let committed = scan_repo(&dir, &base, false).unwrap();
        assert_eq!(committed.len(), 1, "{committed:?}");

        // Sin confirmar: un .env nuevo cuenta por su nombre y por su contenido.
        std::fs::write(dir.join(".env"), "DATABASE_URL=postgres://app:s3cr3tPass@db.host/app\n").unwrap(); // atalaya:permitir (dato de prueba inventado)
        let all = scan_repo(&dir, &base, true).unwrap();
        assert!(all.iter().any(|f| f.file == ".env" && f.line == 0), "{all:?}");
        assert!(all.iter().any(|f| f.file == ".env" && f.line == 1), "{all:?}");
        assert!(report(&all).contains("b.js:1"));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
