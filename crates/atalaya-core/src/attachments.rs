//! Los archivos que adjuntas en el chat: capturas pegadas, imágenes, PDF o cualquier archivo.
//!
//! Se guardan en `adjuntos/<conversación>/` dentro de la carpeta de datos de Atalaya, y cada
//! IA los recibe a su manera:
//! - Claude Code, con la ruta y la carpeta abierta (`--add-dir`): los lee con Read, también
//!   las imágenes y los PDF;
//! - los agentes de terminal, con la ruta: su encierro les deja leer esta carpeta, aunque
//!   no el resto de los datos de Atalaya;
//! - los modelos por API, con las imágenes dentro del mensaje y el texto de los archivos
//!   de texto.
//!
//! Atalaya nunca borra un adjunto por su cuenta.

use crate::Registry;
use anyhow::{Context, Result, bail};
use base64::Engine;
use serde::Serialize;
use std::path::{Path, PathBuf};

/// Lo más grande que se acepta: 25 MB.
pub const MAX_BYTES: u64 = 25 * 1024 * 1024;

/// Lo más grande que se manda como imagen a un modelo por API (el límite de Anthropic).
const MAX_IMAGE_BYTES: u64 = 5 * 1024 * 1024;

/// Lo que se copia del texto de un archivo para un modelo por API.
const MAX_TEXT_CHARS: usize = 30_000;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Attachment {
    /// La ruta del archivo guardado.
    pub path: String,
    /// El nombre con que lo adjuntaste.
    pub name: String,
    /// `imagen`, `pdf`, `texto` o `archivo`.
    pub kind: String,
    pub mime: String,
    pub size: u64,
}

/// Una imagen lista para ir dentro de un mensaje.
#[derive(Debug, Clone, PartialEq, serde::Deserialize, Serialize)]
pub struct Image {
    pub mime: String,
    /// El contenido en base64.
    pub data: String,
}

/// La carpeta de todos los adjuntos, junto a la base.
pub fn root() -> Result<PathBuf> {
    let db = Registry::default_path()?;
    Ok(db.parent().context("la base no tiene carpeta")?.join("adjuntos"))
}

/// Solo letras, números, guiones y puntos: nada de rutas ni nombres raros.
fn clean(name: &str, max: usize) -> String {
    let mut out: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '-' })
        .collect();
    out = out.trim_matches(|c| c == '-' || c == '.').to_string();
    if out.chars().count() > max {
        // Se recorta el comienzo del nombre y se conserva la extensión.
        let ext = Path::new(&out).extension().map(|e| e.to_string_lossy().to_string()).unwrap_or_default();
        let keep = max.saturating_sub(ext.len() + 1).max(1);
        let head: String = out.chars().take(keep).collect();
        out = if ext.is_empty() { head } else { format!("{head}.{ext}") };
    }
    if out.is_empty() { "archivo".into() } else { out }
}

fn mime_of(path: &Path) -> (&'static str, &'static str) {
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "png" => ("imagen", "image/png"),
        "jpg" | "jpeg" => ("imagen", "image/jpeg"),
        "gif" => ("imagen", "image/gif"),
        "webp" => ("imagen", "image/webp"),
        "heic" => ("imagen", "image/heic"),
        "svg" => ("texto", "image/svg+xml"),
        "pdf" => ("pdf", "application/pdf"),
        "txt" | "md" | "csv" | "tsv" | "json" | "log" | "yaml" | "yml" | "toml" | "xml" | "html" | "css" | "js"
        | "jsx" | "ts" | "tsx" | "rs" | "py" | "rb" | "go" | "java" | "kt" | "swift" | "php" | "sql" | "sh" | "env"
        | "ini" | "conf" | "diff" | "patch" => ("texto", "text/plain"),
        _ => ("archivo", "application/octet-stream"),
    }
}

/// Lo que se sabe de un archivo guardado.
fn describe(path: &Path, name: &str) -> Result<Attachment> {
    let size = std::fs::metadata(path)?.len();
    let (mut kind, mime) = mime_of(path);
    // Un archivo sin extensión conocida pero que es texto se trata como texto.
    if kind == "archivo" && size <= 512 * 1024 {
        if let Ok(bytes) = std::fs::read(path) {
            if std::str::from_utf8(&bytes).is_ok() && !bytes.contains(&0) {
                kind = "texto";
            }
        }
    }
    Ok(Attachment { path: path.to_string_lossy().to_string(), name: name.to_string(), kind: kind.into(), mime: mime.into(), size })
}

fn folder_in(root: &Path, conversation: &str) -> Result<PathBuf> {
    let id = clean(conversation, 64);
    let dir = root.join(id);
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Un nombre libre dentro de la carpeta: `captura.png`, `captura-2.png`…
fn free_name(dir: &Path, name: &str) -> PathBuf {
    let name = clean(name, 80);
    let first = dir.join(&name);
    if !first.exists() {
        return first;
    }
    let p = Path::new(&name);
    let stem = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| name.clone());
    let ext = p.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    (2..).map(|n| dir.join(format!("{stem}-{n}{ext}"))).find(|p| !p.exists()).expect("siempre hay un nombre libre")
}

fn save_in(root: &Path, conversation: &str, name: &str, bytes: &[u8]) -> Result<Attachment> {
    if bytes.len() as u64 > MAX_BYTES {
        bail!("«{name}» pesa más de {} MB", MAX_BYTES / 1024 / 1024);
    }
    let dir = folder_in(root, conversation)?;
    let path = free_name(&dir, name);
    std::fs::write(&path, bytes)?;
    describe(&path, name)
}

fn copy_in(root: &Path, conversation: &str, source: &Path) -> Result<Attachment> {
    let meta = std::fs::metadata(source).with_context(|| format!("no se pudo leer {}", source.display()))?;
    let name = source.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "archivo".into());
    if meta.is_dir() {
        bail!("«{name}» es una carpeta: adjunta los archivos, o pídele que trabaje en esa carpeta");
    }
    if meta.len() > MAX_BYTES {
        bail!("«{name}» pesa más de {} MB", MAX_BYTES / 1024 / 1024);
    }
    let dir = folder_in(root, conversation)?;
    let path = free_name(&dir, &name);
    std::fs::copy(source, &path)?;
    describe(&path, &name)
}

/// Guarda lo que pegaste (una captura, por ejemplo).
pub fn save(conversation: &str, name: &str, bytes: &[u8]) -> Result<Attachment> {
    save_in(&root()?, conversation, name, bytes)
}

/// Copia un archivo de tu Mac a los adjuntos de la conversación.
pub fn copy(conversation: &str, source: &Path) -> Result<Attachment> {
    copy_in(&root()?, conversation, source)
}

fn check_in(root: &Path, paths: &[String]) -> Result<Vec<Attachment>> {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    paths
        .iter()
        .map(|p| {
            let path = Path::new(p).canonicalize().with_context(|| format!("el adjunto {p} ya no existe"))?;
            if !path.starts_with(&root) {
                bail!("{p} no es un adjunto de Atalaya");
            }
            let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            describe(&path, &name)
        })
        .collect()
}

/// Los adjuntos de un mensaje: solo rutas dentro de la carpeta de adjuntos.
pub fn check(paths: &[String]) -> Result<Vec<Attachment>> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    check_in(&root()?, paths)
}

/// Lo que se agrega al mensaje para una IA que lee archivos por su cuenta.
pub fn note(list: &[Attachment]) -> String {
    if list.is_empty() {
        return String::new();
    }
    let lines: Vec<String> = list.iter().map(|a| format!("- {} ({}): {}", a.name, a.kind, a.path)).collect();
    format!(
        "\n\nAdjunté estos archivos. Léelos desde su ruta antes de responder (las imágenes y los PDF se ven al leerlos):\n{}",
        lines.join("\n")
    )
}

/// Para un modelo por API: el texto que acompaña al mensaje y las imágenes que van dentro.
pub fn for_api(list: &[Attachment]) -> (String, Vec<Image>) {
    let mut text = String::new();
    let mut images = Vec::new();
    for a in list {
        let path = Path::new(&a.path);
        match a.kind.as_str() {
            "imagen" if a.mime != "image/heic" && a.size <= MAX_IMAGE_BYTES => match std::fs::read(path) {
                Ok(bytes) => {
                    images.push(Image { mime: a.mime.clone(), data: base64::engine::general_purpose::STANDARD.encode(bytes) });
                    text.push_str(&format!("\n\n[Imagen adjunta: {}]", a.name));
                }
                Err(e) => text.push_str(&format!("\n\n[No se pudo leer la imagen {}: {e}]", a.name)),
            },
            "texto" => match std::fs::read_to_string(path) {
                Ok(content) => {
                    let clipped: String = content.chars().take(MAX_TEXT_CHARS).collect();
                    let cut = if clipped.len() < content.len() { "\n(recortado)" } else { "" };
                    text.push_str(&format!("\n\nArchivo adjunto «{}»:\n```\n{clipped}\n```{cut}", a.name));
                }
                Err(e) => text.push_str(&format!("\n\n[No se pudo leer {}: {e}]", a.name)),
            },
            _ => text.push_str(&format!(
                "\n\n[Adjunté «{}» ({}), pero este modelo no puede leerlo desde aquí: para eso, elige Claude Code]",
                a.name, a.kind
            )),
        }
    }
    (text, images)
}

/// Una vista previa pequeña para el chat: la imagen misma en `data:` si no es muy grande.
pub fn preview(a: &Attachment) -> Option<String> {
    if a.kind != "imagen" || a.mime == "image/heic" || a.size > 4 * 1024 * 1024 {
        return None;
    }
    let bytes = std::fs::read(&a.path).ok()?;
    Some(format!("data:{};base64,{}", a.mime, base64::engine::general_purpose::STANDARD.encode(bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("atalaya-adjuntos-{}", std::process::id()));
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let dir = dir.join(n.to_string());
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn saves_with_safe_free_names_and_knows_the_kind() {
        let root = temp();
        let a = save_in(&root, "conv-1", "../../captura de pantalla.png", b"\x89PNG").unwrap();
        assert!(a.path.starts_with(root.join("conv-1").to_str().unwrap()));
        assert!(a.path.ends_with("captura-de-pantalla.png"), "{}", a.path);
        assert_eq!((a.kind.as_str(), a.mime.as_str(), a.size), ("imagen", "image/png", 4));
        let b = save_in(&root, "conv-1", "captura de pantalla.png", b"x").unwrap();
        assert!(b.path.ends_with("captura-de-pantalla-2.png"), "{}", b.path);
        // Un archivo sin extensión que es texto, se lee como texto.
        let c = save_in(&root, "conv-1", "Dockerfile", b"FROM node:22\n").unwrap();
        assert_eq!(c.kind, "texto");
        assert!(save_in(&root, "conv-1", "grande.bin", &vec![0u8; (MAX_BYTES + 1) as usize]).is_err());
    }

    #[test]
    fn only_accepts_its_own_files_and_prepares_them_for_each_ai() {
        let root = temp();
        let png = save_in(&root, "c", "error.png", b"\x89PNG-imagen").unwrap();
        let txt = save_in(&root, "c", "log.txt", b"panic en main.rs:12").unwrap();
        let pdf = save_in(&root, "c", "factura.pdf", b"%PDF-1.4").unwrap();
        let outside = root.parent().unwrap().join("fuera.txt");
        std::fs::write(&outside, "secreto").unwrap();
        assert!(check_in(&root, &[outside.to_string_lossy().to_string()]).is_err());
        let sneaky = format!("{}/c/../../fuera.txt", root.display());
        assert!(check_in(&root, &[sneaky]).is_err());
        let list = check_in(&root, &[png.path.clone(), txt.path.clone(), pdf.path.clone()]).unwrap();
        assert_eq!(list.len(), 3);

        let note = note(&list);
        assert!(note.contains(&png.path) && note.contains("log.txt (texto)"));

        let (text, images) = for_api(&list);
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].mime, "image/png");
        assert!(text.contains("panic en main.rs:12"));
        assert!(text.contains("factura.pdf") && text.contains("elige Claude Code"));
        assert!(preview(&png).unwrap().starts_with("data:image/png;base64,"));
        assert!(preview(&txt).is_none());
    }

    #[test]
    fn copies_files_but_not_folders() {
        let root = temp();
        let source = root.parent().unwrap().join(format!("informe-{}.md", std::process::id()));
        std::fs::write(&source, "# Informe").unwrap();
        let a = copy_in(&root, "c", &source).unwrap();
        assert_eq!(a.kind, "texto");
        assert_eq!(std::fs::read_to_string(&a.path).unwrap(), "# Informe");
        assert!(copy_in(&root, "c", root.parent().unwrap()).is_err());
    }
}
