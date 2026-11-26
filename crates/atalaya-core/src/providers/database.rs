//! Bases de datos propias: PostgreSQL, MySQL/MariaDB y SQLite.
//!
//! Atalaya solo lee: se conecta, consulta la versión y el tamaño, y para respaldar usa
//! las herramientas oficiales (`pg_dump`, `mysqldump`) o la copia en caliente de SQLite.

use super::Outcome;
use crate::checks::{FAIL, OK, WARN};
use crate::connections::{Connection, Link};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Lo que se averigua de una base al conectarse.
#[derive(Debug, Default)]
pub struct Info {
    pub version: String,
    pub database: String,
    pub bytes: Option<u64>,
    pub tables: Option<u64>,
    pub ms: u64,
}

fn url_of(c: &Connection) -> Result<String> {
    c.secret().with_context(|| format!("la conexión «{}» no tiene su URL guardada", c.name))
}

/// La URL sin la contraseña, para mostrarla.
pub fn redact(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else { return "—".into() };
    let rest = match rest.rsplit_once('@') {
        Some((auth, host)) => {
            let user = auth.split(':').next().unwrap_or_default();
            format!("{user}@{host}")
        }
        None => rest.to_string(),
    };
    format!("{scheme}://{}", rest.split('?').next().unwrap_or_default())
}

/// Cambia la base de una URL por la del vínculo, si el proyecto usa otra base del mismo servidor.
fn with_database(url: &str, database: Option<&str>) -> String {
    let Some(db) = database.filter(|d| !d.is_empty()) else { return url.to_string() };
    let (base, query) = match url.split_once('?') {
        Some((b, q)) => (b, Some(q)),
        None => (url, None),
    };
    let after_scheme = base.find("://").map(|i| i + 3).unwrap_or(0);
    let path_start = base[after_scheme..].find('/').map(|i| i + after_scheme);
    let host_part = match path_start {
        Some(i) => &base[..i],
        None => base,
    };
    match query {
        Some(q) => format!("{host_part}/{db}?{q}"),
        None => format!("{host_part}/{db}"),
    }
}

fn postgres_client(url: &str) -> Result<postgres::Client> {
    let mut config: postgres::Config = url.parse().context("la URL de Postgres no es válida")?;
    config.connect_timeout(Duration::from_secs(10));
    // Igual que libpq con sslmode=prefer o require: cifra si el servidor lo ofrece,
    // sin exigir un certificado de una autoridad conocida (los servidores propios
    // suelen tener uno autofirmado).
    let tls = native_tls::TlsConnector::builder()
        .danger_accept_invalid_certs(true)
        .danger_accept_invalid_hostnames(true)
        .build()?;
    config
        .connect(postgres_native_tls::MakeTlsConnector::new(tls))
        .map_err(|e| anyhow::anyhow!("no se pudo conectar a Postgres: {e}"))
}

fn postgres_info(url: &str) -> Result<Info> {
    let start = Instant::now();
    let mut client = postgres_client(url)?;
    let row = client.query_one(
        "select split_part(version(), ' ', 2), current_database()::text, pg_database_size(current_database()),
                (select count(*) from information_schema.tables
                  where table_schema not in ('pg_catalog', 'information_schema'))",
        &[],
    )?;
    Ok(Info {
        version: format!("PostgreSQL {}", row.get::<_, String>(0)),
        database: row.get(1),
        bytes: Some(row.get::<_, i64>(2).max(0) as u64),
        tables: Some(row.get::<_, i64>(3).max(0) as u64),
        ms: start.elapsed().as_millis() as u64,
    })
}

fn mysql_info(url: &str) -> Result<Info> {
    use mysql::prelude::Queryable;
    let start = Instant::now();
    let url = url.replacen("mariadb://", "mysql://", 1);
    let opts = mysql::Opts::from_url(&url).map_err(|e| anyhow::anyhow!("la URL de MySQL no es válida: {e}"))?;
    let opts = mysql::OptsBuilder::from_opts(opts).tcp_connect_timeout(Some(Duration::from_secs(10)));
    let mut conn = mysql::Conn::new(opts).map_err(|e| anyhow::anyhow!("no se pudo conectar a MySQL: {e}"))?;
    let (version, database): (String, Option<String>) =
        conn.query_first("select version(), database()")?.context("MySQL no respondió")?;
    let (bytes, tables): (Option<u64>, Option<u64>) = match &database {
        Some(db) => conn
            .exec_first(
                "select cast(coalesce(sum(data_length + index_length), 0) as unsigned), count(*)
                 from information_schema.tables where table_schema = ?",
                (db,),
            )?
            .unwrap_or((None, None)),
        None => (None, None),
    };
    Ok(Info {
        version: if version.to_lowercase().contains("mariadb") { format!("MariaDB {version}") } else { format!("MySQL {version}") },
        database: database.unwrap_or_else(|| "sin base elegida".into()),
        bytes,
        tables,
        ms: start.elapsed().as_millis() as u64,
    })
}

fn sqlite_path(c: &Connection) -> Result<PathBuf> {
    let path = c.config_str("path").context("la conexión no tiene archivo")?;
    let path = PathBuf::from(shellexpand_home(&path));
    if !path.is_file() {
        bail!("no existe el archivo {}", path.display());
    }
    Ok(path)
}

/// `~/x` → `/Users/tu/x`.
pub(crate) fn shellexpand_home(path: &str) -> String {
    match (path.strip_prefix("~/"), dirs::home_dir()) {
        (Some(rest), Some(home)) => home.join(rest).display().to_string(),
        _ => path.to_string(),
    }
}

fn sqlite_info(path: &Path) -> Result<(Info, String)> {
    let start = Instant::now();
    let conn = rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("no se pudo abrir {}", path.display()))?;
    let integrity: String = conn.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
    let tables: i64 = conn.query_row("select count(*) from sqlite_master where type = 'table'", [], |r| r.get(0))?;
    let tables = tables as u64;
    let version: String = conn.query_row("select sqlite_version()", [], |r| r.get(0))?;
    Ok((
        Info {
            version: format!("SQLite {version}"),
            database: path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
            bytes: std::fs::metadata(path).ok().map(|m| m.len()),
            tables: Some(tables),
            ms: start.elapsed().as_millis() as u64,
        },
        integrity,
    ))
}

fn size_label(bytes: Option<u64>) -> String {
    match bytes {
        Some(b) if b >= 1_073_741_824 => format!("{:.1} GB", b as f64 / 1_073_741_824.0),
        Some(b) => format!("{:.1} MB", b as f64 / 1_048_576.0),
        None => "tamaño desconocido".into(),
    }
}

fn describe(info: &Info) -> String {
    let tables = info.tables.map(|t| format!(" · {t} tablas")).unwrap_or_default();
    format!("{} · base {} · {}{tables} · {} ms", info.version, info.database, size_label(info.bytes), info.ms)
}

pub fn test(c: &Connection) -> Result<String> {
    match c.provider.as_str() {
        "postgres" => Ok(format!("Conectado: {}", describe(&postgres_info(&url_of(c)?)?))),
        "mysql" => Ok(format!("Conectado: {}", describe(&mysql_info(&url_of(c)?)?))),
        "sqlite" => {
            let (info, integrity) = sqlite_info(&sqlite_path(c)?)?;
            if integrity != "ok" {
                bail!("el archivo tiene problemas de integridad: {integrity}");
            }
            Ok(format!("Abierta: {}", describe(&info)))
        }
        other => bail!("'{other}' no es una base propia"),
    }
}

/// Revisión de una base vinculada a un proyecto.
pub fn check(c: &Connection, link: &Link) -> Outcome {
    let database = link.config_str("database");
    let result = match c.provider.as_str() {
        "postgres" => url_of(c).and_then(|u| postgres_info(&with_database(&u, database.as_deref()))).map(|i| (i, None)),
        "mysql" => url_of(c).and_then(|u| mysql_info(&with_database(&u, database.as_deref()))).map(|i| (i, None)),
        "sqlite" => sqlite_path(c).and_then(|p| sqlite_info(&p)).map(|(i, integrity)| (i, Some(integrity))),
        other => Err(anyhow::anyhow!("'{other}' no es una base propia")),
    };
    match result {
        Ok((info, integrity)) => {
            let details = json!({
                "version": info.version, "base": info.database, "bytes": info.bytes,
                "tablas": info.tables, "ms": info.ms, "integridad": integrity,
            });
            if let Some(problem) = integrity.filter(|i| i != "ok") {
                return (FAIL, format!("El archivo tiene problemas de integridad: {problem}"), details);
            }
            if info.ms > 3000 {
                (WARN, format!("Responde lento: {}", describe(&info)), details)
            } else {
                (OK, describe(&info), details)
            }
        }
        Err(e) => (FAIL, "No responde".into(), json!({ "error": format!("{e:#}") })),
    }
}

/// Busca una herramienta en el PATH y en las ubicaciones habituales de Homebrew y Postgres.app.
pub(crate) fn find_tool(name: &str) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    dirs.extend(
        [
            "/opt/homebrew/bin",
            "/usr/local/bin",
            "/opt/homebrew/opt/libpq/bin",
            "/opt/homebrew/opt/mysql-client/bin",
            "/Applications/Postgres.app/Contents/Versions/latest/bin",
            "/usr/bin",
        ]
        .map(PathBuf::from),
    );
    dirs.into_iter().map(|d| d.join(name)).find(|p| p.is_file())
}

/// Respalda una base propia en `dir`. Devuelve una descripción de lo guardado.
pub fn backup(c: &Connection, link: &Link, dir: &Path) -> Result<Value> {
    let database = link.config_str("database");
    let file_stem = crate::registry::slugify(&c.name);
    match c.provider.as_str() {
        "postgres" => {
            let tool = find_tool("pg_dump").context("no se encontró pg_dump; instálalo (brew install libpq) para respaldar Postgres")?;
            let url = with_database(&url_of(c)?, database.as_deref());
            let config: postgres::Config = url.parse().context("la URL de Postgres no es válida")?;
            let file = dir.join(format!("{file_stem}.dump"));
            // Los datos de conexión van por las variables de entorno de libpq, no como
            // argumentos, para que la clave no aparezca en la lista de procesos.
            let mut command = std::process::Command::new(tool);
            command.args(["--format=custom", "--no-owner", "--file"]).arg(&file);
            if let Some(postgres::config::Host::Tcp(host)) = config.get_hosts().first() {
                command.env("PGHOST", host);
            }
            if let Some(port) = config.get_ports().first() {
                command.env("PGPORT", port.to_string());
            }
            if let Some(user) = config.get_user() {
                command.env("PGUSER", user);
            }
            if let Some(password) = config.get_password() {
                command.env("PGPASSWORD", String::from_utf8_lossy(password).to_string());
            }
            if let Some(db) = config.get_dbname() {
                command.env("PGDATABASE", db);
            }
            command.env(
                "PGSSLMODE",
                match config.get_ssl_mode() {
                    postgres::config::SslMode::Disable => "disable",
                    postgres::config::SslMode::Require => "require",
                    _ => "prefer",
                },
            );
            let out = command.output()?;
            if !out.status.success() {
                bail!("pg_dump falló: {}", String::from_utf8_lossy(&out.stderr).trim());
            }
            Ok(json!({ "origen": "postgres", "conexion": c.name, "archivo": file.file_name().map(|n| n.to_string_lossy().to_string()) }))
        }
        "mysql" => {
            let tool = find_tool("mysqldump").context("no se encontró mysqldump; instálalo (brew install mysql-client) para respaldar MySQL")?;
            let url = with_database(&url_of(c)?.replacen("mariadb://", "mysql://", 1), database.as_deref());
            let opts = mysql::Opts::from_url(&url).map_err(|e| anyhow::anyhow!("la URL de MySQL no es válida: {e}"))?;
            let db = opts.get_db_name().context("la URL de MySQL no dice qué base respaldar")?.to_string();
            let file = dir.join(format!("{file_stem}.sql"));
            let out = std::process::Command::new(tool)
                .arg(format!("--host={}", opts.get_ip_or_hostname()))
                .arg(format!("--port={}", opts.get_tcp_port()))
                .arg(format!("--user={}", opts.get_user().unwrap_or_default()))
                .args(["--single-transaction", "--routines", "--result-file"])
                .arg(&file)
                .arg(&db)
                // La clave va por variable de entorno, nunca como argumento.
                .env("MYSQL_PWD", opts.get_pass().unwrap_or_default())
                .output()?;
            if !out.status.success() {
                bail!("mysqldump falló: {}", String::from_utf8_lossy(&out.stderr).trim());
            }
            Ok(json!({ "origen": "mysql", "conexion": c.name, "archivo": file.file_name().map(|n| n.to_string_lossy().to_string()) }))
        }
        "sqlite" => {
            let source = rusqlite::Connection::open_with_flags(sqlite_path(c)?, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
            let file = dir.join(format!("{file_stem}.db"));
            let mut target = rusqlite::Connection::open(&file)?;
            let backup = rusqlite::backup::Backup::new(&source, &mut target)?;
            backup.run_to_completion(256, Duration::from_millis(5), None)?;
            Ok(json!({ "origen": "sqlite", "conexion": c.name, "archivo": file.file_name().map(|n| n.to_string_lossy().to_string()) }))
        }
        other => bail!("'{other}' no se respalda como base propia"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_passwords() {
        assert_eq!(redact("postgres://ana:secreto@db.casa:5432/tienda?sslmode=require"), "postgres://ana@db.casa:5432/tienda");
        assert_eq!(redact("mysql://root@localhost/x"), "mysql://root@localhost/x");
        assert_eq!(redact("nada"), "—");
    }

    #[test]
    fn swaps_the_database_name() {
        assert_eq!(with_database("postgres://u:p@h:5432/a?x=1", Some("b")), "postgres://u:p@h:5432/b?x=1");
        assert_eq!(with_database("postgres://u:p@h:5432", Some("b")), "postgres://u:p@h:5432/b");
        assert_eq!(with_database("postgres://u:p@h/a", None), "postgres://u:p@h/a");
    }

    fn sqlite_connection(path: &Path) -> Connection {
        Connection {
            id: "s".into(),
            kind: "base".into(),
            provider: "sqlite".into(),
            name: "Datos de prueba".into(),
            config: json!({ "path": path.display().to_string() }),
            secret_account: None,
            created_at: String::new(),
        }
    }

    #[test]
    fn checks_and_backs_up_sqlite() {
        let dir = std::env::temp_dir().join(format!("atalaya-sqlite-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("datos.db");
        let db = rusqlite::Connection::open(&file).unwrap();
        db.execute_batch("create table clientes (id integer primary key, nombre text); insert into clientes (nombre) values ('Ana'), ('Luis');")
            .unwrap();
        drop(db);

        let c = sqlite_connection(&file);
        assert!(test(&c).unwrap().contains("1 tablas"));
        let link = Link { slug: "x".into(), role: "base".into(), connection_id: "s".into(), config: json!({}) };
        let (status, summary, _) = check(&c, &link);
        assert_eq!(status, OK, "{summary}");

        let out = dir.join("respaldo");
        std::fs::create_dir_all(&out).unwrap();
        let meta = backup(&c, &link, &out).unwrap();
        let copy = rusqlite::Connection::open(out.join(meta["archivo"].as_str().unwrap())).unwrap();
        let rows: i64 = copy.query_row("select count(*) from clientes", [], |r| r.get(0)).unwrap();
        assert_eq!(rows, 2);

        let missing = sqlite_connection(&dir.join("no-existe.db"));
        assert_eq!(check(&missing, &link).0, FAIL);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
