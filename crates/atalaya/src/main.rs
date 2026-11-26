//! `atalaya`: línea de comandos y servidor MCP sobre el núcleo.

mod mcp;

use anyhow::{Context, Result, bail};
use atalaya_core::{Registry, checks, connections, discover, flags, github, mail, project_context, secrets, status};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "atalaya", version, about = "Vigía de tus proyectos")]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Registra (o actualiza) un proyecto a partir de su carpeta local.
    Add {
        path: PathBuf,
        /// Identificador corto; por defecto, el nombre de la carpeta.
        #[arg(long)]
        slug: Option<String>,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        description: Option<String>,
        /// URL de producción.
        #[arg(long)]
        url: Option<String>,
        #[arg(long)]
        notes: Option<String>,
        /// ID del proyecto en Vercel (prj_…), si la carpeta no está enlazada.
        #[arg(long)]
        vercel_project: Option<String>,
        /// ID del equipo de Vercel (team_…).
        #[arg(long)]
        vercel_team: Option<String>,
        /// Referencia del proyecto de Supabase (20 caracteres).
        #[arg(long)]
        supabase_ref: Option<String>,
    },
    /// Lista los proyectos registrados.
    List,
    /// Muestra el contexto completo de un proyecto, igual que lo ve la IA.
    Show { slug: String },
    /// Quita un proyecto del registro (no toca su carpeta).
    Remove { slug: String },
    /// Administra las llaves de servicios externos, guardadas en el Llavero.
    Secret {
        #[command(subcommand)]
        action: SecretCmd,
    },
    /// Conexiones: cuentas, bases, servidores e IA que usa Atalaya. Puede haber varias
    /// del mismo proveedor.
    Connections {
        #[command(subcommand)]
        action: ConnectionsCmd,
    },
    /// IA: modelos de cada conexión, preguntas de prueba, consumo, precios y presupuestos.
    Ai {
        #[command(subcommand)]
        action: AiCmd,
    },
    /// Skills que reciben todas las IA, e instrucciones comunes.
    Skills {
        #[command(subcommand)]
        action: SkillsCmd,
    },
    /// La memoria que reciben todas las IA: lo propuesto y lo aprobado.
    Memoria {
        #[command(subcommand)]
        action: MemoriaCmd,
    },
    /// Tareas programadas: encargos de solo lectura que corren cada día, semana o tantas horas.
    Tareas {
        #[command(subcommand)]
        action: TareasCmd,
    },
    /// Reacciones: cuando una revisión pasa a falla, una IA la diagnostica.
    Reacciones {
        #[command(subcommand)]
        action: ReaccionesCmd,
    },
    /// La misma pregunta a varias IA a la vez, para comparar sus respuestas.
    Comparar {
        pregunta: String,
        /// Una IA (id de conexión o claude-code); repítelo para cada una, de 2 a 4.
        #[arg(long = "ia", required = true)]
        ias: Vec<String>,
        /// El proyecto del que se pregunta.
        #[arg(long)]
        project: Option<String>,
    },
    /// Comandos guardados que se llaman desde el chat con /nombre.
    Comandos {
        #[command(subcommand)]
        action: ComandosCmd,
    },
    /// Servidores MCP externos (Supabase, GitHub, Context7…) que reciben todas las IA.
    Mcps {
        #[command(subcommand)]
        action: McpsCmd,
    },
    /// Carpetas donde guardas tus proyectos: Atalaya los busca ahí.
    Roots {
        #[command(subcommand)]
        action: RootsCmd,
    },
    /// Busca proyectos (carpetas con .git) en las carpetas elegidas.
    Scan {
        /// Registra los que encuentre y crea los vínculos que propone.
        #[arg(long)]
        add: bool,
    },
    /// Une un proyecto con una conexión en un rol: repo, hosting, base o respaldo.
    Link {
        slug: String,
        /// repo, hosting, base o respaldo.
        role: String,
        /// ID de la conexión (ver `atalaya connections list`).
        connection: String,
        /// Lo propio del proyecto en ese servicio, como clave=valor: ref=…, site_id=…,
        /// repo=grupo/nombre, database=…, project_id=…
        #[arg(long = "set", value_name = "CLAVE=VALOR")]
        set: Vec<String>,
    },
    /// Quita el vínculo entre un proyecto y una conexión.
    Unlink { slug: String, role: String, connection: String },
    /// Interruptores de funciones: prender y apagar partes de una app sin tocar código.
    Flags {
        #[command(subcommand)]
        action: FlagsCmd,
    },
    /// Configura y prueba el correo de los avisos (también se hace en Conexiones → Avisos).
    Email {
        #[command(subcommand)]
        action: EmailCmd,
    },
    /// GitHub: ver los repos de la cuenta, crear uno nuevo o vigilar uno sin carpeta local.
    Github {
        #[command(subcommand)]
        action: GithubCmd,
    },
    /// Claude Platform: gasto y uso de la API de Claude en el mes.
    Claude {
        #[command(subcommand)]
        action: ClaudeCmd,
    },
    /// Comprueba todo lo que Atalaya necesita: permisos, herramientas, llaves y servicio.
    Doctor,
    /// Ronda: el validador (Ajustes → IA) revisa todo y deja propuestas en Decisiones.
    Round {
        /// Activa o desactiva la ronda diaria automática: `on` u `off`.
        #[arg(long)]
        auto: Option<String>,
    },
    /// Administración: los controles que cada app ofrece por su puerta para Atalaya.
    Admin {
        #[command(subcommand)]
        action: AdminCmd,
    },
    /// Radar: vigila páginas de la competencia y avisa cuando cambian.
    Radar {
        #[command(subcommand)]
        action: RadarCmd,
    },
    /// Respalda ahora las bases de datos (de un proyecto o de todos).
    Backup { slug: Option<String> },
    /// El historial del chat: listar, buscar, ver y exportar conversaciones.
    Conversaciones {
        #[command(subcommand)]
        action: ConversacionesCmd,
    },
    /// Busca llaves, tokens y archivos sensibles en lo que cambió una carpeta (lo mismo que se revisa antes de cada pull request).
    Secretos {
        /// Carpeta del repositorio (por omisión, la actual).
        #[arg(default_value = ".")]
        carpeta: PathBuf,
        /// Desde qué commit o rama mirar; incluye lo que aún no confirmas.
        #[arg(long, default_value = "HEAD")]
        desde: String,
    },
    /// Avisos al teléfono por Telegram, para las fallas graves.
    Telegram {
        #[command(subcommand)]
        action: TelegramCmd,
    },
    /// Firebase: conecta la cuenta de servicio con la que Atalaya lee tus proyectos.
    Firebase {
        #[command(subcommand)]
        action: FirebaseCmd,
    },
    /// Lista los proyectos de Supabase visibles con la llave guardada.
    SupabaseProjects,
    /// Ejecuta ahora todas las revisiones (de un proyecto o de todos).
    Check {
        slug: Option<String>,
        /// Envía notificaciones si algo cambió de estado.
        #[arg(long)]
        notify: bool,
    },
    /// Pone al constructor (Ajustes → IA) a trabajar una mejora aceptada, hasta dejarla en un pull request.
    Work {
        /// Número de la mejora.
        id: i64,
    },
    /// Instala, quita o consulta el vigía como servicio en segundo plano de macOS.
    Service {
        #[command(subcommand)]
        action: ServiceCmd,
    },
    /// Vigila sin parar: repite cada revisión cuando le toca y avisa de los cambios.
    Watch,
    /// Imprime un reporte con el último estado de todos los proyectos.
    Report {
        /// Además lo envía por correo.
        #[arg(long)]
        email: bool,
    },
    /// Arranca el servidor MCP por stdio (lo lanza Claude Code u Orca).
    Mcp {
        /// Solo las herramientas que leen (así lo reciben los agentes de terminal).
        #[arg(long)]
        read_only: bool,
    },
    /// Uso interno: conecta un agente encerrado con el MCP de Atalaya.
    #[command(hide = true)]
    McpPuente { file: PathBuf },
}

#[derive(Subcommand)]
enum AiCmd {
    /// Lista los modelos de una conexión de IA.
    Models { connection: String },
    /// Hace una pregunta corta a un modelo (queda anotada en el consumo).
    Ask {
        connection: String,
        prompt: String,
        #[arg(long)]
        model: Option<String>,
    },
    /// Consumo del mes por conexión, modelo y tarea.
    Usage,
    /// Fija el precio de un modelo en dólares por millón de tokens.
    Price { connection: String, model: String, input: f64, output: f64 },
    /// Fija el presupuesto mensual de una conexión en dólares (0 lo quita).
    Budget { connection: String, usd: f64 },
    /// Tope de gasto de cada encargo en dólares (0 lo quita). Sin monto, muestra el actual.
    Tope { usd: Option<f64> },
    /// Muestra los roles: constructor, revisor, validador y asistente.
    Roles,
    /// Agentes de terminal que usan tu cuenta: si están instalados y con sesión.
    Clis,
    /// Abre la Terminal para iniciar sesión en un agente (codex, gemini-cli, kimi…).
    Login { agent: String },
    /// Valida agentes de verdad con una pregunta corta, encerrados y en solo lectura. Sin agente, todos los instalados.
    Verify { agent: Option<String> },
    /// Modelos que corren en este Mac (Ollama, LM Studio, Jan, llama.cpp).
    Local,
    /// Elige el modelo de un agente (como qwen-code) en vez de su cuenta. Sin número, lista
    /// los modelos posibles, gratis primero.
    Model {
        agent: String,
        /// El número de la lista.
        choice: Option<usize>,
        /// Vuelve a usar la cuenta del agente.
        #[arg(long)]
        cuenta: bool,
    },
    /// Asigna un rol: candidatos en orden (ids de conexión o claude-code). Sin candidatos lo deja vacío.
    Role {
        role: String,
        candidates: Vec<String>,
        /// gratis (gratis primero), primero, barato o rotar.
        #[arg(long, default_value = "gratis")]
        strategy: String,
    },
}

#[derive(Subcommand)]
enum TareasCmd {
    /// Las tareas y lo último que corrieron.
    List,
    /// Agrega una tarea. El encargo puede ser un comando guardado: "/resumen-semana faro".
    Add {
        name: String,
        encargo: String,
        /// dia, semana u horas.
        #[arg(long, default_value = "dia")]
        cada: String,
        /// Hora local, HH:MM (para dia y semana).
        #[arg(long, default_value = "09:00")]
        hora: String,
        /// Día de la semana (lunes, martes…), para semana.
        #[arg(long)]
        dia: Option<String>,
        /// Cada cuántas horas, para horas.
        #[arg(long)]
        horas: Option<i64>,
        #[arg(long)]
        project: Option<String>,
        /// La IA (id de conexión o claude-code); sin ella, la primera del validador.
        #[arg(long)]
        ia: Option<String>,
    },
    /// Corre una tarea ahora.
    Run { id: i64 },
    /// Pausa una tarea.
    Pause { id: i64 },
    /// Reanuda una tarea.
    Resume { id: i64 },
    /// Quita una tarea (su historial queda).
    Remove { id: i64 },
}

#[derive(Subcommand)]
enum ReaccionesCmd {
    /// Las reacciones.
    List,
    /// Agrega una reacción. Sin --tipo, reacciona a cualquier revisión que falle.
    Add {
        name: String,
        /// Revisión que la dispara (despliegue, sitio, github…); repítelo para varias.
        #[arg(long = "tipo")]
        tipos: Vec<String>,
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        ia: Option<String>,
        /// Además del aviso, deja el diagnóstico como mejora propuesta.
        #[arg(long)]
        mejora: bool,
    },
    /// Quita una reacción.
    Remove { id: i64 },
}

#[derive(Subcommand)]
enum MemoriaCmd {
    /// Lista lo propuesto y lo aprobado.
    List,
    /// Agrega una memoria (queda aprobada).
    Add {
        text: String,
        /// Solo para este proyecto; sin él, para todos.
        #[arg(long)]
        project: Option<String>,
    },
    /// Aprueba una propuesta, con el texto ajustado si lo das.
    Approve { id: i64, text: Option<String> },
    /// Descarta (u olvida) una memoria. No se borra: queda descartada.
    Discard { id: i64 },
}

#[derive(Subcommand)]
enum ComandosCmd {
    /// Lista los comandos guardados.
    List,
    /// Guarda (o cambia) un comando. La plantilla recibe lo que escribas después en {texto}.
    Add {
        name: String,
        template: String,
        #[arg(long, default_value = "")]
        description: String,
    },
    /// Agrega los de ejemplo que falten.
    Examples,
    /// Quita un comando.
    Remove { name: String },
}

#[derive(Subcommand)]
enum McpsCmd {
    /// Lista los servidores y los conocidos que se pueden agregar.
    List,
    /// Agrega (o cambia) un servidor. Queda apagado hasta probarlo y activarlo.
    Add {
        name: String,
        /// Uno conocido: supabase, github, context7 o playwright.
        #[arg(long)]
        preset: Option<String>,
        /// El programa con sus argumentos, por ejemplo "npx -y @upstash/context7-mcp".
        #[arg(long)]
        command: Option<String>,
        /// La dirección de un servidor web (https://…).
        #[arg(long)]
        url: Option<String>,
        /// Variable sin secretos para el programa, como CLAVE=valor; se repite.
        #[arg(long = "env")]
        env: Vec<String>,
        /// La variable donde el programa lee su llave (por ejemplo SUPABASE_ACCESS_TOKEN).
        #[arg(long)]
        secret_var: Option<String>,
        /// Qué hace, en una línea.
        #[arg(long, default_value = "")]
        description: String,
        /// No pedir la llave (deja la que había).
        #[arg(long)]
        no_secret: bool,
    },
    /// Lo arranca y lista sus herramientas.
    Test { name: String },
    /// Lo activa. Sin --project ni --ai rige en todos lados.
    Enable {
        name: String,
        #[arg(long = "project")]
        projects: Vec<String>,
        #[arg(long = "ai")]
        ais: Vec<String>,
    },
    /// Lo apaga.
    Disable { name: String },
    /// Lo quita de Atalaya (su llave queda en el Llavero).
    Remove { name: String },
}

#[derive(Subcommand)]
enum SkillsCmd {
    /// Lista las skills instaladas y dónde rigen.
    List,
    /// Instala las skills de una carpeta o de un repo de git. Quedan apagadas.
    Add { source: String },
    /// Muestra qué trae una skill: sus archivos y su SKILL.md.
    Show { name: String },
    /// Activa una skill. Sin --project ni --ai rige en todos lados.
    Enable {
        name: String,
        /// Solo en estos proyectos (slug); se repite.
        #[arg(long = "project")]
        projects: Vec<String>,
        /// Solo para estas IA (`claude-code` o el id de la conexión); se repite.
        #[arg(long = "ai")]
        ais: Vec<String>,
    },
    /// Apaga una skill.
    Disable { name: String },
    /// Quita una skill: su carpeta va a la Papelera.
    Remove { name: String },
    /// Muestra o cambia las instrucciones para todas las IA (o las de un proyecto).
    Instructions {
        /// El proyecto; sin él, las de todas las IA.
        #[arg(long)]
        project: Option<String>,
        /// El texto nuevo; vacío las borra. Sin él, muestra las actuales.
        text: Option<String>,
    },
}

#[derive(Subcommand)]
enum RootsCmd {
    /// Lista las carpetas elegidas.
    List,
    /// Agrega una carpeta.
    Add { path: PathBuf },
    /// Quita una carpeta de la lista (no toca la carpeta ni los proyectos).
    Remove { path: PathBuf },
}

#[derive(Subcommand)]
enum ConnectionsCmd {
    /// Lista las conexiones y los proveedores disponibles.
    List,
    /// Muestra los proveedores que se pueden conectar y qué pide cada uno.
    Providers,
    /// Agrega (o, con --id, edita) una conexión. Si el proveedor pide llave, la pregunta
    /// sin mostrarla.
    Add {
        /// Proveedor, por ejemplo postgres, netlify, gitlab, servidor, openai…
        provider: String,
        #[arg(long, default_value = "")]
        name: String,
        /// Configuración como clave=valor; se puede repetir.
        #[arg(long = "set", value_name = "CLAVE=VALOR")]
        set: Vec<String>,
        /// Edita la conexión con este ID en vez de crear una.
        #[arg(long)]
        id: Option<String>,
        /// No preguntar la llave (deja la que había).
        #[arg(long)]
        no_secret: bool,
    },
    /// Prueba una conexión.
    Test { id: String },
    /// Quita una conexión y sus vínculos. La llave queda en el Llavero salvo con --forget-secret.
    Remove {
        id: String,
        #[arg(long)]
        forget_secret: bool,
    },
}

fn key_values(pairs: &[String]) -> Result<serde_json::Value> {
    let mut map = serde_json::Map::new();
    for pair in pairs {
        let (k, v) = pair.split_once('=').with_context(|| format!("'{pair}' no es clave=valor"))?;
        map.insert(k.trim().to_string(), serde_json::Value::String(v.trim().to_string()));
    }
    Ok(serde_json::Value::Object(map))
}

#[derive(Subcommand)]
enum SecretCmd {
    /// Pide la llave en la terminal (sin mostrarla) y la guarda.
    Set { service: String },
    /// Muestra qué servicios tienen llave guardada, sin revelar ninguna.
    List,
    /// Borra la llave de un servicio.
    Remove { service: String },
}

#[derive(Subcommand)]
enum FlagsCmd {
    /// Lista los interruptores de un proyecto.
    List { slug: String },
    /// Crea un interruptor (apagado) o cambia su descripción.
    Add {
        slug: String,
        name: String,
        /// Qué hace este interruptor, en palabras simples.
        #[arg(long, default_value = "")]
        desc: String,
        /// Crearlo ya prendido.
        #[arg(long)]
        on: bool,
    },
    /// Prende un interruptor.
    On { slug: String, name: String },
    /// Apaga un interruptor.
    Off { slug: String, name: String },
    /// Cambia el texto asociado a un interruptor (por ejemplo, el mensaje de un aviso).
    Text { slug: String, name: String, text: String },
    /// Elimina un interruptor.
    Remove { slug: String, name: String },
    /// Conecta la app al almacén: guarda la variable GLOBAL_CONFIG en su proyecto de Vercel.
    Connect {
        slug: String,
        /// Además la escribe en el .env.local de la carpeta del proyecto, para desarrollo.
        #[arg(long)]
        local: bool,
    },
    /// Muestra los últimos cambios hechos a los interruptores de un proyecto.
    History { slug: String },
}

fn print_admin(schema: &serde_json::Value) {
    let text = |v: &serde_json::Value| match v {
        serde_json::Value::Null => "—".to_string(),
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    println!("{} · protocolo {}", text(&schema["app"]), schema["protocol"]);
    for section in schema["sections"].as_array().into_iter().flatten() {
        println!("\n== {} ({})", text(&section["title"]), text(&section["id"]));
        for item in section["items"].as_array().into_iter().flatten() {
            println!("\n  {}  [{}]", text(&item["title"]), text(&item["id"]));
            if let Some(detail) = item["detail"].as_str() {
                println!("  {detail}");
            }
            for control in item["controls"].as_array().into_iter().flatten() {
                println!("    {:<28} {:<22} {}", text(&control["key"]), text(&control["value"]), text(&control["label"]));
            }
        }
    }
}

fn print_flags(all: &flags::Flags) {
    if all.is_empty() {
        println!("Sin interruptores.");
    }
    for (name, flag) in all {
        let state = if flag.on { "PRENDIDO" } else { "apagado " };
        println!("{state}  {name:<28} {}", flag.desc);
        if !flag.value.is_empty() {
            println!("          texto: {}", flag.value);
        }
    }
}

#[derive(Subcommand)]
enum GithubCmd {
    /// Lista los repos de tu cuenta y marca cuáles vigila Atalaya.
    Repos,
    /// Crea un repo nuevo en tu cuenta (privado por defecto).
    Create {
        name: String,
        #[arg(long, default_value = "")]
        desc: String,
        /// Crearlo público.
        #[arg(long)]
        public: bool,
    },
    /// Vigila un repo de GitHub aunque no tenga carpeta en esta máquina.
    Watch {
        /// `dueño/repo`.
        repo: String,
        #[arg(long)]
        slug: Option<String>,
        /// URL de producción.
        #[arg(long)]
        url: Option<String>,
    },
}

#[derive(Subcommand)]
enum ClaudeCmd {
    /// Consulta Claude Platform ahora y muestra el gasto del mes por modelo.
    Status,
    /// Fija el presupuesto mensual en dólares; avisa al llegar al 80 % y al 100 %.
    Budget { usd: f64 },
}

#[derive(Subcommand)]
enum AdminCmd {
    /// Conecta la puerta de una app: genera su llave, la guarda en el Llavero y en Vercel.
    Connect {
        slug: String,
        /// Dónde vive la app, por ejemplo admin.tuapp.com.
        url: String,
        /// Genera una llave nueva aunque ya exista una.
        #[arg(long)]
        rotate: bool,
    },
    /// Muestra los controles de la app y sus valores actuales.
    Show { slug: String },
    /// Cambia un control. El valor se lee como JSON si se puede (true, 40, null) y si no como texto.
    Set {
        slug: String,
        section: String,
        item: String,
        key: String,
        value: String,
    },
    /// Muestra la llave, para ponerla a mano en una app que no está en Vercel.
    Key { slug: String },
}

#[derive(Subcommand)]
enum RadarCmd {
    /// Agrega una página de la competencia a vigilar para un proyecto.
    Add { slug: String, name: String, url: String },
    /// Lista las páginas vigiladas y los últimos cambios.
    List,
    /// Revisa ahora todas las páginas.
    Check,
    /// Deja de vigilar una página (por su número).
    Remove { id: i64 },
}

#[derive(Subcommand)]
enum ConversacionesCmd {
    /// Las últimas conversaciones.
    List {
        #[arg(long, default_value_t = 20)]
        limite: usize,
    },
    /// Busca un texto en todas las conversaciones.
    Buscar {
        texto: String,
        #[arg(long)]
        project: Option<String>,
    },
    /// Muestra una conversación (basta el comienzo de su identificador).
    Ver { id: String },
    /// La exporta a Markdown; sin --salida, la muestra.
    Exportar {
        id: String,
        /// Archivo donde guardarla. No reemplaza uno que ya exista.
        #[arg(long)]
        salida: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum TelegramCmd {
    /// Guarda el chat que recibe los avisos y pide la llave del bot.
    Setup {
        /// Identificador del chat (te lo da @userinfobot en Telegram).
        chat: String,
    },
    /// Envía un mensaje de prueba.
    Test,
    /// Si las decisiones nuevas llegan con botones para aprobarlas o descartarlas (si/no). Sin valor, dice cómo está.
    Decisiones { valor: Option<String> },
    /// Escucha los botones y comandos en primer plano (el Vigía ya lo hace solo).
    Escuchar,
}

#[derive(Subcommand)]
enum FirebaseCmd {
    /// Guarda en el Llavero la llave de cuenta de servicio (el archivo .json de Firebase).
    Connect { file: PathBuf },
}

#[derive(Subcommand)]
enum ServiceCmd {
    /// Lo instala y lo arranca; se inicia solo en cada inicio de sesión.
    Install,
    /// Lo detiene y lo quita.
    Uninstall,
    /// Dice si está corriendo.
    Status,
}

#[derive(Subcommand)]
enum EmailCmd {
    /// Guarda la cuenta de Gmail que envía y pide su contraseña de aplicación.
    Setup {
        /// Tu dirección de Gmail.
        address: String,
        /// A quién enviar; por defecto, a la misma dirección.
        #[arg(long)]
        to: Option<String>,
        /// Hora local (0-23) del resumen diario; por defecto, 8.
        #[arg(long)]
        hour: Option<u32>,
    },
    /// Envía un correo de prueba.
    Test,
    /// Muestra la configuración actual, sin revelar la contraseña.
    Status,
}

/// Pide un secreto sin mostrarlo. Sin terminal (en un script) lo lee de la entrada estándar.
fn read_secret(label: &str) -> Result<String> {
    use std::io::IsTerminal;
    if std::io::stdin().is_terminal() {
        return Ok(rpassword::prompt_password(format!("Pega {label} y pulsa Enter (no se mostrará): "))?);
    }
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(line)
}

fn ask_secret(service: &str, label: &str) -> Result<()> {
    let value = read_secret(label)?;
    // Google muestra la contraseña de aplicación en grupos separados por espacios.
    let value: String = value.split_whitespace().collect();
    if !secrets::set(service, &value)? {
        bail!("no se pudo guardar en el Llavero");
    }
    Ok(())
}

fn known_service(service: &str) -> Result<()> {
    if !secrets::SERVICES.contains(&service) {
        bail!("servicio desconocido '{service}'; disponibles: {}", secrets::SERVICES.join(", "));
    }
    Ok(())
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Cmd::Add {
            path,
            slug,
            name,
            description,
            url,
            notes,
            vercel_project,
            vercel_team,
            supabase_ref,
        } => {
            let registry = Registry::open_default()?;
            let options = discover::AddOptions {
                slug,
                name,
                description,
                url,
                notes,
                vercel_project,
                vercel_team,
                supabase_ref,
            };
            let project = discover::add_local(&registry, &path, options)?;
            println!("Registrado: {} ({})", project.name, project.slug);
        }
        Cmd::List => {
            for p in Registry::open_default()?.list()? {
                println!(
                    "{:<16} {:<24} {}",
                    p.slug,
                    p.repo.unwrap_or_default(),
                    p.production_url.unwrap_or_default()
                );
            }
        }
        Cmd::Show { slug } => {
            let ctx = project_context(&Registry::open_default()?, &slug)?;
            println!("{}", serde_json::to_string_pretty(&ctx)?);
        }
        Cmd::Remove { slug } => {
            if !Registry::open_default()?.remove(&slug)? {
                bail!("no existe el proyecto '{slug}'");
            }
            println!("Quitado: {slug}");
        }
        Cmd::Secret { action } => match action {
            SecretCmd::Set { service } => {
                known_service(&service)?;
                ask_secret(&service, &format!("la llave de {service}"))?;
                println!("Llave de {service} guardada en el Llavero.");
            }
            SecretCmd::List => {
                for service in secrets::SERVICES {
                    let state = if secrets::get(service).is_some() { "guardada" } else { "falta" };
                    println!("{service:<10} {state}");
                }
            }
            SecretCmd::Remove { service } => {
                known_service(&service)?;
                if !secrets::remove(&service)? {
                    bail!("no había llave guardada para {service}");
                }
                println!("Llave de {service} borrada.");
            }
        },
        Cmd::Connections { action } => {
            let registry = Registry::open_default()?;
            match action {
                ConnectionsCmd::List => {
                    let list = connections::list(&registry)?;
                    let items = list["connections"].as_array().cloned().unwrap_or_default();
                    if items.is_empty() {
                        println!("Sin conexiones. Agrega una con `atalaya connections add <proveedor>`.");
                    }
                    for c in items {
                        let test = &c["last_test"];
                        let state = match test["ok"].as_bool() {
                            Some(true) => "probada",
                            Some(false) => "falló",
                            None => "sin probar",
                        };
                        let used: Vec<&str> = c["used_by"].as_array().into_iter().flatten().filter_map(|v| v.as_str()).collect();
                        println!(
                            "{:<22} {:<18} {:<24} {:<10} {}",
                            c["id"].as_str().unwrap_or_default(),
                            c["provider"].as_str().unwrap_or_default(),
                            c["name"].as_str().unwrap_or_default(),
                            state,
                            if used.is_empty() { String::new() } else { format!("usada por {}", used.join(", ")) }
                        );
                    }
                }
                ConnectionsCmd::Providers => {
                    for p in connections::PROVIDERS {
                        let fields: Vec<String> = p
                            .fields
                            .iter()
                            .map(|f| if f.required { format!("{}*", f.key) } else { f.key.to_string() })
                            .collect();
                        println!(
                            "{:<10} {:<20} {}{}{}",
                            p.kind,
                            p.id,
                            p.label,
                            if fields.is_empty() { String::new() } else { format!(" · campos: {}", fields.join(", ")) },
                            p.secret.map(|s| format!(" · llave: {s}")).unwrap_or_default()
                        );
                    }
                }
                ConnectionsCmd::Add { provider, name, set, id, no_secret } => {
                    let p = connections::provider(&provider).with_context(|| {
                        format!("proveedor desconocido '{provider}'; mira `atalaya connections providers`")
                    })?;
                    let secret = match p.secret {
                        Some(label) if !no_secret => {
                            let value = read_secret(label)?;
                            Some(value.trim().to_string()).filter(|v| !v.is_empty())
                        }
                        _ => None,
                    };
                    let input = connections::Input { id, provider, name, config: key_values(&set)?, secret };
                    let saved = connections::save(&registry, &input)?;
                    println!("Guardada: {} ({})", saved.name, saved.id);
                    match connections::test(&registry, &saved.id) {
                        Ok(message) => println!("Prueba: {message}"),
                        Err(e) => println!("La prueba falló: {e:#}"),
                    }
                }
                ConnectionsCmd::Test { id } => println!("{}", connections::test(&registry, &id)?),
                ConnectionsCmd::Remove { id, forget_secret } => {
                    connections::remove(&registry, &id, forget_secret)?;
                    println!("Conexión {id} quitada{}.", if forget_secret { " y su llave borrada" } else { "" });
                }
            }
        }
        Cmd::Ai { action } => {
            let registry = Registry::open_default()?;
            let get = |id: &str| {
                connections::get(&registry, id)?.with_context(|| format!("no existe la conexión '{id}'"))
            };
            match action {
                AiCmd::Models { connection } => {
                    for m in atalaya_core::ai::models(&registry, &get(&connection)?)? {
                        println!("{m}");
                    }
                }
                AiCmd::Ask { connection, prompt, .. } if get(&connection)?.kind == "agente" => {
                    let c = get(&connection)?;
                    println!("{}", atalaya_core::clis::ask(&registry, &c, "", &prompt, "prueba", None)?);
                }
                AiCmd::Clis => {
                    for s in atalaya_core::clis::states(&registry)? {
                        let session = match s.session {
                            "lista" => "sesión iniciada",
                            "sin-sesion" => "falta iniciar sesión",
                            "desconocida" => "instalado (sesión sin comprobar)",
                            _ => "no instalado",
                        };
                        let used = if s.connection.is_some() { " · en Atalaya" } else { "" };
                        println!("{:<18} {session}{used}", s.cli.label);
                        if let Some(source) = &s.model_source {
                            println!("{:<18} usa {source}", "");
                        } else if let Some(free) = s.cli.free {
                            println!("{:<18} {free}", "");
                        }
                        if let Some(old) = &s.legacy {
                            println!("{:<18} tienes {old}, que ya no funciona: instala {}", "", s.cli.label);
                        }
                        if !s.installed {
                            if let Some(cmd) = s.cli.install {
                                println!("{:<18} instalar: {cmd}", "");
                            }
                        } else if s.session == "sin-sesion" {
                            println!("{:<18} atalaya ai login {}", "", s.cli.id);
                        }
                    }
                }
                AiCmd::Verify { agent } => {
                    let ids: Vec<&str> = match &agent {
                        Some(a) => vec![a.as_str()],
                        None => atalaya_core::clis::CATALOG
                            .iter()
                            .filter(|c| atalaya_core::clis::find(c).is_some())
                            .map(|c| c.id)
                            .collect(),
                    };
                    let results: Vec<_> = ids
                        .iter()
                        .map(|id| {
                            let (id, path) = (id.to_string(), registry.path());
                            std::thread::spawn(move || (id.clone(), Registry::open(&path).and_then(|r| atalaya_core::clis::verify(&r, &id))))
                        })
                        .collect();
                    for handle in results {
                        let (id, result) = handle.join().map_err(|_| anyhow::anyhow!("falló una validación"))?;
                        match result {
                            Ok(text) => println!("✓ {id:<14} {text}"),
                            Err(e) => println!("✗ {id:<14} {e:#}"),
                        }
                    }
                }
                AiCmd::Model { agent, choice, cuenta } => {
                    if cuenta {
                        atalaya_core::clis::set_model(&registry, &agent, None)?;
                        println!("{agent} vuelve a usar su cuenta.");
                    } else {
                        let sources = atalaya_core::clis::model_sources(&registry)?;
                        match choice {
                            None => {
                                if sources.is_empty() {
                                    println!("No hay modelos: enciende Ollama o LM Studio, o guarda una conexión de modelos.");
                                }
                                for (i, src) in sources.iter().enumerate() {
                                    let free = if src.free { " · gratis" } else { "" };
                                    println!("{:>2}. {}{free}", i + 1, src.label);
                                }
                                println!("Elige con: atalaya ai model {agent} <número>");
                            }
                            Some(n) => {
                                let src = sources.get(n.wrapping_sub(1)).context("ese número no está en la lista")?;
                                atalaya_core::clis::set_model(&registry, &agent, Some(src))?;
                                println!("{agent} usa {}.", src.label);
                            }
                        }
                    }
                }
                AiCmd::Login { agent } => {
                    atalaya_core::clis::login(&agent)?;
                    println!("Se abrió la Terminal. Termina ahí y luego revisa con «atalaya ai clis».");
                }
                AiCmd::Local => {
                    for f in atalaya_core::local::detect() {
                        let status = if f.running {
                            format!("encendido en {} · {}", f.base_url, if f.models.is_empty() { "sin modelos".into() } else { f.models.join(", ") })
                        } else if f.installed {
                            let open = if f.app_open { "abierto, pero con el servidor apagado" } else { "instalado, pero apagado" };
                            let models = if f.downloaded.is_empty() { "sin modelos descargados".to_string() } else { format!("descargados: {}", f.downloaded.join(", ")) };
                            format!("{open} · {models}\n{:<10} {}", "", f.hint)
                        } else {
                            format!("no instalado ({})", f.url)
                        };
                        println!("{:<10} {status}", f.label);
                    }
                }
                AiCmd::Ask { connection, prompt, model } => {
                    let req = atalaya_core::ai::Request {
                        model,
                        messages: vec![atalaya_core::ai::Message::user(prompt)],
                        max_tokens: 1024,
                        task: "prueba".into(),
                        ..Default::default()
                    };
                    let reply = atalaya_core::ai::complete(&registry, &get(&connection)?, &req)?;
                    println!("{}", reply.text);
                    eprintln!(
                        "\n{} · {} tokens de entrada · {} de salida · {}",
                        reply.model,
                        reply.input_tokens.map_or("?".into(), |t| t.to_string()),
                        reply.output_tokens.map_or("?".into(), |t| t.to_string()),
                        reply.cost_usd.map_or("precio sin definir".into(), |c| format!("US$ {c:.4}"))
                    );
                }
                AiCmd::Usage => {
                    let s = atalaya_core::ai::usage_summary(&registry)?;
                    println!("Gasto del mes con precio conocido: US$ {:.2}", s["month_usd"].as_f64().unwrap_or(0.0));
                    let share = &s["by_ai"];
                    println!(
                        "Quién hizo el trabajo ({} llamadas, {}% gratis):",
                        share["calls"],
                        share["free_pct"].as_f64().unwrap_or(0.0)
                    );
                    for a in share["ais"].as_array().into_iter().flatten() {
                        let tasks: Vec<String> = a["tasks"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .map(|t| format!("{} {}", t["task"].as_str().unwrap_or_default(), t["calls"]))
                            .collect();
                        println!(
                            "  {:<28} {:>5}%  {:>5} llamadas  {:<9} {}",
                            a["name"].as_str().unwrap_or_default(),
                            a["calls_pct"].as_f64().unwrap_or(0.0),
                            a["calls"],
                            a["billing"].as_str().unwrap_or_default(),
                            tasks.join(", ")
                        );
                    }
                    println!("Por modelo:");
                    for r in s["by_model"].as_array().into_iter().flatten() {
                        let cost = match (r["cost_usd"].as_f64(), r["billing"].as_str()) {
                            (Some(c), _) => format!("US$ {c:.4}"),
                            (None, Some("cuenta")) => "tu cuenta".into(),
                            (None, Some("gratis")) => "gratis".into(),
                            _ => "precio sin definir".into(),
                        };
                        println!(
                            "  {:<22} {:<32} {:>5} llamadas {:>9} entrada {:>9} salida  {}",
                            r["connection"].as_str().unwrap_or_default(),
                            r["model"].as_str().unwrap_or_default(),
                            r["calls"],
                            r["input_tokens"],
                            r["output_tokens"],
                            cost
                        );
                    }
                }
                AiCmd::Price { connection, model, input, output } => {
                    atalaya_core::ai::set_price(&registry, &connection, &model, input, output)?;
                    println!("Precio de {model}: US$ {input} entrada y US$ {output} salida por millón de tokens.");
                }
                AiCmd::Roles => {
                    let s = atalaya_core::orchestrator::summary(&registry)?;
                    for role in s["roles"].as_array().into_iter().flatten() {
                        println!("{} ({})", role["label"].as_str().unwrap_or_default(), role["strategy"].as_str().unwrap_or_default());
                        let list = role["candidates"].as_array().cloned().unwrap_or_default();
                        if list.is_empty() {
                            println!("  sin asignar");
                        }
                        for c in list {
                            let state = match c["reason"].as_str() {
                                Some(why) => format!("no disponible: {why}"),
                                None => "listo".into(),
                            };
                            println!("  {:<24} {:<36} {state}", c["id"].as_str().unwrap_or_default(), c["name"].as_str().unwrap_or_default());
                        }
                        // Lo que se puede agregar, gratis primero.
                        let chosen: Vec<&str> = role["candidates"].as_array().into_iter().flatten().filter_map(|c| c["id"].as_str()).collect();
                        let options: Vec<String> = role["options"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter(|o| o["id"].as_str().is_some_and(|id| !chosen.contains(&id)))
                            .map(|o| format!("{}{}", o["id"].as_str().unwrap_or_default(), if o["free"] == true { " (gratis)" } else { "" }))
                            .collect();
                        if !options.is_empty() {
                            println!("  se puede agregar: {}", options.join(", "));
                        }
                    }
                }
                AiCmd::Role { role, candidates, strategy } => {
                    atalaya_core::orchestrator::set_config(
                        &registry,
                        &role,
                        &atalaya_core::orchestrator::RoleConfig { candidates, strategy },
                    )?;
                    println!("Rol {role} guardado.");
                }
                AiCmd::Budget { connection, usd } => {
                    get(&connection)?;
                    atalaya_core::ai::set_budget(&registry, &connection, Some(usd))?;
                    println!("{}", if usd > 0.0 { format!("Presupuesto: US$ {usd} al mes.") } else { "Presupuesto quitado.".into() });
                }
                AiCmd::Tope { usd } => {
                    if let Some(usd) = usd {
                        atalaya_core::ai::set_task_cap(&registry, Some(usd))?;
                    }
                    match atalaya_core::ai::task_cap(&registry)? {
                        Some(cap) => println!(
                            "Tope por encargo: US$ {cap}. Claude Code lo respeta con --max-budget-usd y los modelos por API \
                             con precio conocido acortan la respuesta; los agentes de terminal no informan su gasto."
                        ),
                        None => println!("Sin tope por encargo."),
                    }
                }
            }
        }
        Cmd::Tareas { action } => {
            use atalaya_core::automations::{self as auto, Schedule, WEEKDAYS};
            let registry = Registry::open_default()?;
            match action {
                TareasCmd::List => {
                    for s in auto::schedules(&registry)? {
                        let when = match s.every.as_str() {
                            "semana" => format!("cada {} a las {}", WEEKDAYS[s.weekday.unwrap_or(1) as usize], s.at),
                            "horas" => format!("cada {} horas", s.hours.unwrap_or(0)),
                            _ => format!("cada día a las {}", s.at),
                        };
                        println!(
                            "#{:<3} {:<24} {:<28} {}{}",
                            s.id,
                            s.name,
                            when,
                            s.slug.as_deref().unwrap_or("todos"),
                            if s.enabled { "" } else { " · pausada" }
                        );
                    }
                    for r in auto::runs(&registry, 10)?.into_iter().filter(|r| r.source == "tarea") {
                        println!("  {} {} ({}): {}", r.at, r.name, r.ai, r.text.lines().next().unwrap_or_default());
                    }
                }
                TareasCmd::Add { name, encargo, cada, hora, dia, horas, project, ia } => {
                    let weekday = match dia {
                        Some(d) => Some(
                            WEEKDAYS.iter().position(|w| w.eq_ignore_ascii_case(d.trim()) || w.replace('é', "e") == d.trim().to_lowercase())
                                .with_context(|| format!("«{d}» no es un día de la semana"))? as i64,
                        ),
                        None => None,
                    };
                    let s = auto::save_schedule(
                        &registry,
                        &Schedule { name, prompt: encargo, every: cada, at: hora, weekday, hours: horas, slug: project, ai: ia, enabled: true, ..Default::default() },
                    )?;
                    println!("Tarea #{} guardada. La corre la Vigía.", s.id);
                }
                TareasCmd::Run { id } => {
                    let run = auto::run_now(&registry, id)?;
                    println!("{} ({}):\n{}", run.name, run.ai, run.text);
                }
                TareasCmd::Pause { id } => auto::set_schedule_enabled(&registry, id, false)?,
                TareasCmd::Resume { id } => auto::set_schedule_enabled(&registry, id, true)?,
                TareasCmd::Remove { id } => auto::remove_schedule(&registry, id)?,
            }
        }
        Cmd::Reacciones { action } => {
            use atalaya_core::automations::{self as auto, Reaction};
            let registry = Registry::open_default()?;
            match action {
                ReaccionesCmd::List => {
                    for x in auto::reactions(&registry)? {
                        let kinds = if x.kinds.is_empty() { "cualquier revisión".to_string() } else { x.kinds.join(", ") };
                        println!(
                            "#{:<3} {:<24} {:<32} {}{}{}",
                            x.id,
                            x.name,
                            kinds,
                            x.slug.as_deref().unwrap_or("todos"),
                            if x.improvement { " · deja mejora" } else { "" },
                            if x.enabled { "" } else { " · pausada" }
                        );
                    }
                }
                ReaccionesCmd::Add { name, tipos, project, ia, mejora } => {
                    let x = auto::save_reaction(
                        &registry,
                        &Reaction { name, kinds: tipos, slug: project, ai: ia, improvement: mejora, enabled: true, ..Default::default() },
                    )?;
                    println!("Reacción #{} guardada. La Vigía la usa cuando una revisión pasa a falla.", x.id);
                }
                ReaccionesCmd::Remove { id } => auto::remove_reaction(&registry, id)?,
            }
        }
        Cmd::Comparar { pregunta, ias, project } => {
            let registry = Registry::open_default()?;
            let answers = atalaya_core::compare::ask_many(&registry, &ias, &pregunta, project.as_deref(), &|_, a| {
                eprintln!("· {} respondió en {} s", a.name, a.seconds);
            })?;
            for a in answers {
                let cost = match a.cost_usd {
                    Some(c) => format!("US$ {c:.4}"),
                    None => "costo no informado".into(),
                };
                println!("\n=== {} ({} s · {cost}){}\n{}", a.name, a.seconds, if a.ok { "" } else { " · no respondió" }, a.text);
            }
        }
        Cmd::Memoria { action } => {
            use atalaya_core::memories;
            let registry = Registry::open_default()?;
            match action {
                MemoriaCmd::List => {
                    for m in memories::list(&registry)? {
                        println!("#{:<4} {:<10} {:<12} {}", m.id, m.status, m.slug.as_deref().unwrap_or("todos"), m.text);
                    }
                }
                MemoriaCmd::Add { text, project } => {
                    let m = memories::add(&registry, project.as_deref(), &text)?;
                    println!("Guardada: #{}", m.id);
                }
                MemoriaCmd::Approve { id, text } => {
                    memories::decide(&registry, id, true, text.as_deref())?;
                    println!("Aprobada: #{id}");
                }
                MemoriaCmd::Discard { id } => {
                    memories::decide(&registry, id, false, None)?;
                    println!("Descartada: #{id}");
                }
            }
        }
        Cmd::Comandos { action } => {
            use atalaya_core::commands;
            let registry = Registry::open_default()?;
            match action {
                ComandosCmd::List => {
                    for c in commands::list(&registry)? {
                        println!("/{:<20} {}", c.name, c.description);
                    }
                }
                ComandosCmd::Add { name, template, description } => {
                    let c = commands::save(&registry, &name, &description, &template)?;
                    println!("Guardado: /{}", c.name);
                }
                ComandosCmd::Examples => println!("Agregados: {}", commands::add_examples(&registry)?),
                ComandosCmd::Remove { name } => {
                    commands::remove(&registry, &name)?;
                    println!("Quitado: /{name}");
                }
            }
        }
        Cmd::Mcps { action } => {
            use atalaya_core::mcp_servers::{self as mcps, PRESETS, Server};
            let registry = Registry::open_default()?;
            let scope = |l: &Option<Vec<String>>| l.as_ref().map(|v| v.join(", ")).unwrap_or_else(|| "todos".into());
            match action {
                McpsCmd::List => {
                    for s in mcps::list(&registry)? {
                        let target = s.url.clone().unwrap_or_else(|| format!("{} {}", s.command.clone().unwrap_or_default(), s.args.join(" ")));
                        println!(
                            "{:<16} {:<9} {:<8} {} herramientas · proyectos: {} · IA: {}{}\n  {target}",
                            s.name,
                            s.kind,
                            if s.enabled { "activo" } else { "apagado" },
                            s.tools.len(),
                            scope(&s.projects),
                            scope(&s.agents),
                            if s.has_secret { " · con llave" } else { "" },
                        );
                    }
                    println!("\nConocidos: {}", PRESETS.iter().map(|p| p.id).collect::<Vec<_>>().join(", "));
                }
                McpsCmd::Add { name, preset, command, url, env, secret_var, description, no_secret } => {
                    let base = preset.as_deref().map(|id| PRESETS.iter().find(|p| p.id == id).with_context(|| format!("no conozco «{id}»"))).transpose()?;
                    let mut words = command.as_deref().map(|c| c.split_whitespace().map(String::from).collect::<Vec<_>>()).unwrap_or_default();
                    let (cmd, args) = if words.is_empty() {
                        (base.and_then(|p| p.command).map(String::from), base.map(|p| p.args.iter().map(|a| a.to_string()).collect()).unwrap_or_default())
                    } else {
                        let first = words.remove(0);
                        (Some(first), words)
                    };
                    let url = url.or_else(|| base.and_then(|p| p.url).map(String::from));
                    let mut vars = std::collections::BTreeMap::new();
                    for pair in env {
                        let (k, v) = pair.split_once('=').with_context(|| format!("«{pair}» no es CLAVE=valor"))?;
                        vars.insert(k.trim().to_string(), v.trim().to_string());
                    }
                    let kind = if url.is_some() && cmd.is_none() { mcps::WEB } else { mcps::PROGRAM };
                    let server = Server {
                        name,
                        description: if description.is_empty() { base.map(|p| p.description.to_string()).unwrap_or_default() } else { description },
                        kind: kind.into(),
                        command: cmd,
                        args,
                        url,
                        env: vars,
                        secret_var: secret_var.or_else(|| base.and_then(|p| p.secret_var).map(String::from)),
                        enabled: false,
                        projects: None,
                        agents: None,
                        tools: vec![],
                        tested_at: None,
                        installed_at: String::new(),
                        has_secret: false,
                    };
                    let wants_key = server.secret_var.is_some() || kind == mcps::WEB;
                    let key = if wants_key && !no_secret {
                        let typed = read_secret("la llave del servidor (vacío: sin llave)")?;
                        Some(typed.trim().to_string()).filter(|k| !k.is_empty())
                    } else {
                        None
                    };
                    let saved = mcps::save(&registry, &server, key.as_deref())?;
                    println!("Guardado: {} (apagado). Pruébalo con `atalaya mcps test {}`.", saved.name, saved.name);
                }
                McpsCmd::Test { name } => {
                    let tools = mcps::test(&registry, &name)?;
                    println!("{} herramientas: {}", tools.len(), tools.join(", "));
                }
                McpsCmd::Enable { name, projects, ais } => {
                    let some = |v: Vec<String>| (!v.is_empty()).then_some(v);
                    let s = mcps::configure(&registry, &name, true, some(projects), some(ais))?;
                    println!("Activo: {} · proyectos: {} · IA: {}", s.name, scope(&s.projects), scope(&s.agents));
                }
                McpsCmd::Disable { name } => {
                    let s = mcps::get(&registry, &name)?.with_context(|| format!("no hay un servidor «{name}»"))?;
                    mcps::configure(&registry, &name, false, s.projects, s.agents)?;
                    println!("Apagado: {name}");
                }
                McpsCmd::Remove { name } => {
                    mcps::remove(&registry, &name)?;
                    println!("Quitado: {name} (su llave queda en el Llavero)");
                }
            }
        }
        Cmd::Skills { action } => {
            use atalaya_core::skills;
            let registry = Registry::open_default()?;
            let scope = |l: &Option<Vec<String>>| l.as_ref().map(|v| v.join(", ")).unwrap_or_else(|| "todos".into());
            match action {
                SkillsCmd::List => {
                    let list = skills::list(&registry)?;
                    if list.is_empty() {
                        println!("Sin skills. Instala una con `atalaya skills add <carpeta o URL>`.");
                    }
                    for s in list {
                        println!(
                            "{} [{}] {}\n    proyectos: {} · IA: {}",
                            s.name,
                            if s.enabled { "activa" } else { "apagada" },
                            s.description,
                            scope(&s.projects),
                            scope(&s.agents)
                        );
                    }
                }
                SkillsCmd::Add { source } => {
                    let path = PathBuf::from(&source);
                    let added = if path.is_dir() {
                        skills::install_folder(&registry, &path.canonicalize()?, &source)?
                    } else {
                        skills::install_git(&registry, &source)?
                    };
                    for s in added {
                        println!("Instalada (apagada): {} — revísala con `atalaya skills show {}`", s.name, s.name);
                    }
                }
                SkillsCmd::Show { name } => {
                    let r = skills::review(&registry, &name)?;
                    for f in &r.files {
                        println!("{}{} ({} B)", f.path, if f.script { "  ← programa" } else { "" }, f.size);
                    }
                    println!("\n{}", r.text);
                }
                SkillsCmd::Enable { name, projects, ais } => {
                    let some = |v: Vec<String>| (!v.is_empty()).then_some(v);
                    let s = skills::configure(&registry, &name, true, some(projects), some(ais))?;
                    println!("Activa: {} · proyectos: {} · IA: {}", s.name, scope(&s.projects), scope(&s.agents));
                }
                SkillsCmd::Disable { name } => {
                    let s = skills::get(&registry, &name)?.with_context(|| format!("no hay una skill «{name}»"))?;
                    skills::configure(&registry, &name, false, s.projects, s.agents)?;
                    println!("Apagada: {name}");
                }
                SkillsCmd::Remove { name } => {
                    skills::remove(&registry, &name)?;
                    println!("Quitada: {name} (su carpeta está en la Papelera)");
                }
                SkillsCmd::Instructions { project, text } => match text {
                    Some(text) => {
                        skills::set_instructions(&registry, project.as_deref(), &text)?;
                        println!("Guardadas.");
                    }
                    None => {
                        let text = skills::instructions(&registry, project.as_deref())?;
                        println!("{}", if text.is_empty() { "(sin instrucciones)" } else { &text });
                    }
                },
            }
        }
        Cmd::Roots { action } => {
            let registry = Registry::open_default()?;
            match action {
                RootsCmd::List => {
                    let list = discover::roots(&registry)?;
                    if list.is_empty() {
                        println!("Sin carpetas. Agrega una con `atalaya roots add <carpeta>`.");
                    }
                    for root in list {
                        println!("{}", root.display());
                    }
                }
                RootsCmd::Add { path } => println!("Agregada: {}", discover::add_root(&registry, &path)?.display()),
                RootsCmd::Remove { path } => {
                    if !discover::remove_root(&registry, &path)? {
                        bail!("esa carpeta no estaba en la lista");
                    }
                    println!("Quitada de la lista.");
                }
            }
        }
        Cmd::Scan { add } => {
            let registry = Registry::open_default()?;
            if discover::roots(&registry)?.is_empty() {
                bail!("no hay carpetas elegidas; agrega una con `atalaya roots add <carpeta>`");
            }
            let found = discover::scan(&registry)?;
            let new: Vec<String> = found.iter().filter(|f| f.registered.is_none()).map(|f| f.path.clone()).collect();
            for f in &found {
                let state = match &f.registered {
                    Some(slug) => format!("registrado como {slug}"),
                    None => "nuevo".to_string(),
                };
                println!("{:<40} {:<24} {}", f.path, state, f.services.join(", "));
                for s in &f.suggestions {
                    println!("    propone {} → {} ({})", s.role, s.connection, s.reason);
                }
            }
            if add {
                let (projects, links) = discover::register(&registry, &new, true)?;
                println!("Registrados {} proyectos y {links} vínculos.", projects.len());
            } else if !new.is_empty() {
                println!("{} nuevos. Regístralos con `atalaya scan --add`.", new.len());
            }
        }
        Cmd::Link { slug, role, connection, set } => {
            let registry = Registry::open_default()?;
            connections::link(&registry, &slug, &role, &connection, &key_values(&set)?)?;
            println!("{slug} vinculado a {connection} como {role}.");
        }
        Cmd::Unlink { slug, role, connection } => {
            if !connections::unlink(&Registry::open_default()?, &slug, &role, &connection)? {
                bail!("no había ese vínculo");
            }
            println!("Vínculo quitado.");
        }
        Cmd::Flags { action } => {
            let registry = Registry::open_default()?;
            let source = "terminal";
            match action {
                FlagsCmd::List { slug } => print_flags(&flags::list(&registry, &slug)?),
                FlagsCmd::Add { slug, name, desc, on } => {
                    print_flags(&flags::add(&registry, &slug, &name, &desc, on, source)?)
                }
                FlagsCmd::On { slug, name } => {
                    print_flags(&flags::set(&registry, &slug, &name, true, source)?)
                }
                FlagsCmd::Off { slug, name } => {
                    print_flags(&flags::set(&registry, &slug, &name, false, source)?)
                }
                FlagsCmd::Text { slug, name, text } => {
                    print_flags(&flags::set_value(&registry, &slug, &name, &text, source)?)
                }
                FlagsCmd::Connect { slug, local } => {
                    let env_file = match (local, registry.get(&slug)?.and_then(|p| p.local_path)) {
                        (true, Some(path)) => Some(PathBuf::from(path).join(".env.local")),
                        (true, None) => bail!("el proyecto no tiene carpeta local"),
                        (false, _) => None,
                    };
                    flags::connect(&registry, &slug, env_file.as_deref())?;
                    println!("{slug} conectado: variable GLOBAL_CONFIG guardada en Vercel.");
                    if let Some(file) = env_file {
                        println!("También escrita en {}", file.display());
                    }
                    println!("La app la tomará en su próximo despliegue.");
                }
                FlagsCmd::Remove { slug, name } => {
                    print_flags(&flags::remove(&registry, &slug, &name, source)?)
                }
                FlagsCmd::History { slug } => {
                    for (at, what, source) in registry.events(&slug, 30)? {
                        println!("{at} UTC  {what}  ({source})");
                    }
                }
            }
        }
        Cmd::Email { action } => {
            let registry = Registry::open_default()?;
            match action {
                EmailCmd::Setup { address, to, hour } => {
                    if !address.contains('@') {
                        bail!("'{address}' no parece una dirección de correo");
                    }
                    if hour.is_some_and(|h| h > 23) {
                        bail!("la hora debe estar entre 0 y 23");
                    }
                    registry.set_setting(mail::ADDRESS, &address)?;
                    if let Some(to) = to {
                        registry.set_setting(mail::TO, &to)?;
                    }
                    if let Some(hour) = hour {
                        registry.set_setting(mail::REPORT_HOUR, &hour.to_string())?;
                    }
                    if secrets::get("gmail").is_none() {
                        ask_secret("gmail", "la contraseña de aplicación de Gmail")?;
                    }
                    println!("Correo configurado. Pruébalo con: atalaya email test");
                }
                EmailCmd::Test => {
                    mail::send(
                        &registry,
                        "Atalaya: correo de prueba",
                        "Si lees esto, Atalaya ya puede enviarte alertas y el resumen diario.",
                    )?;
                    println!("Correo de prueba enviado.");
                }
                EmailCmd::Status => {
                    let show = |key| registry.setting(key).map(|v| v.unwrap_or_else(|| "—".into()));
                    println!("Cuenta que envía:  {}", show(mail::ADDRESS)?);
                    println!("Destinatario:      {}", registry.setting(mail::TO)?.map_or_else(|| "la misma cuenta".into(), |t| t));
                    println!("Resumen diario:    {}:00", registry.setting(mail::REPORT_HOUR)?.unwrap_or_else(|| "8".into()));
                    println!("Contraseña:        {}", if secrets::get("gmail").is_some() { "guardada" } else { "falta" });
                }
            }
        }
        Cmd::Github { action } => {
            let registry = Registry::open_default()?;
            match action {
                GithubCmd::Repos => {
                    let watched: Vec<String> = registry
                        .list()?
                        .into_iter()
                        .filter_map(|p| p.repo.map(|r| r.to_lowercase()))
                        .collect();
                    for r in github::repos()? {
                        let mark = if watched.contains(&r.full_name.to_lowercase()) { "vigilado" } else { "" };
                        let kind = if r.private { "privado" } else { "público" };
                        let pushed = r.pushed_at.as_deref().map(|d| &d[..10.min(d.len())]).unwrap_or("-");
                        println!("{:<44} {kind:<8} {pushed:<11} {mark}", r.full_name);
                    }
                }
                GithubCmd::Create { name, desc, public } => {
                    let repo = github::create(&name, &desc, !public)?;
                    println!("Repo creado: {}", repo.url);
                }
                GithubCmd::Watch { repo, slug, url } => {
                    let project = github::watch(&registry, &repo, slug, url)?;
                    println!("Vigilando {repo} como '{}'.", project.slug);
                }
            }
        }
        Cmd::Claude { action } => {
            let registry = Registry::open_default()?;
            match action {
                ClaudeCmd::Status => {
                    let s = atalaya_core::claude::refresh(&registry)?;
                    println!("Mes {}: US$ {:.2} en {} solicitudes", s["month"].as_str().unwrap_or_default(), s["month_usd"].as_f64().unwrap_or(0.0), s["requests"]);
                    for m in s["models"].as_array().into_iter().flatten() {
                        println!("  {:<24} US$ {:>9.2}  {:>8} solicitudes", m["model"].as_str().unwrap_or_default(), m["usd"].as_f64().unwrap_or(0.0), m["requests"]);
                    }
                    if let Some(budget) = registry.setting(atalaya_core::claude::BUDGET)? {
                        println!("Presupuesto mensual: US$ {budget}");
                    }
                }
                ClaudeCmd::Budget { usd } => {
                    if usd < 0.0 {
                        bail!("el presupuesto no puede ser negativo");
                    }
                    registry.set_setting(atalaya_core::claude::BUDGET, &usd.to_string())?;
                    println!("Presupuesto mensual fijado en US$ {usd:.2}.");
                }
            }
        }
        Cmd::Doctor => {
            let registry = Registry::open_default()?;
            let items = status::doctor(&registry)?;
            let mut missing = 0;
            for item in items.as_array().into_iter().flatten() {
                let ok = item["ok"].as_bool().unwrap_or(false);
                println!("{} {:<34} {}", if ok { "✓" } else { "✗" }, item["name"].as_str().unwrap_or_default(), item["detail"].as_str().unwrap_or_default());
                if !ok {
                    missing += 1;
                    println!("    → {}", item["fix"].as_str().unwrap_or_default());
                }
            }
            println!("\n{}", if missing == 0 { "Todo listo.".to_string() } else { format!("{missing} cosas por resolver.") });
        }
        Cmd::Round { auto } => {
            let registry = Registry::open_default()?;
            if let Some(auto) = auto {
                let on = matches!(auto.as_str(), "on" | "si" | "sí");
                registry.set_setting("round_auto", if on { "si" } else { "no" })?;
                println!("Ronda diaria automática: {}.", if on { "activada" } else { "desactivada" });
            } else {
                println!("El agente está haciendo la ronda…");
                let summary = atalaya_core::round::run(&registry, &|_, step| println!("   {step}"))?;
                println!("\n{summary}\n");
                for d in atalaya_core::decisions::list(&registry, true)? {
                    println!("#{:<3} {:<12} [{}] {}", d.id, d.slug, d.action, d.title);
                }
            }
        }
        Cmd::Admin { action } => {
            let registry = Registry::open_default()?;
            match action {
                AdminCmd::Connect { slug, url, rotate } => {
                    let done = atalaya_core::admin::connect(&registry, &slug, &url, rotate)?;
                    println!("Puerta de {slug}: {}", done.url);
                    println!("{}", done.note);
                }
                AdminCmd::Show { slug } => print_admin(&atalaya_core::admin::schema(&registry, &slug)?),
                AdminCmd::Set { slug, section, item, key, value } => {
                    let value: serde_json::Value =
                        serde_json::from_str(&value).unwrap_or(serde_json::Value::String(value));
                    let change = serde_json::json!({ "section": section, "item": item, "key": key, "value": value });
                    println!("{}", atalaya_core::admin::apply(&registry, &slug, &change, "terminal")?);
                }
                AdminCmd::Key { slug } => match atalaya_core::admin::key(&slug) {
                    Some(key) => println!("{key}"),
                    None => bail!("'{slug}' no tiene llave: atalaya admin connect {slug} <dirección>"),
                },
            }
        }
        Cmd::Radar { action } => {
            let registry = Registry::open_default()?;
            match action {
                RadarCmd::Add { slug, name, url } => {
                    let id = atalaya_core::radar::add(&registry, &slug, &name, &url)?;
                    let _ = atalaya_core::radar::check(&registry, id);
                    println!("Vigilando {name} para {slug}. La primera lectura queda como referencia.");
                }
                RadarCmd::List => {
                    for c in atalaya_core::radar::competitors(&registry)? {
                        let state = c.last_error.unwrap_or_else(|| c.last_checked.unwrap_or_else(|| "sin revisar".into()));
                        println!("#{:<3} {:<12} {:<22} {}  ({state})", c.id, c.slug, c.name, c.url);
                    }
                    for e in atalaya_core::radar::events(&registry)?.iter().take(10) {
                        println!("{} {}: {}", e.at, e.competitor, e.summary);
                    }
                }
                RadarCmd::Check => {
                    for c in atalaya_core::radar::competitors(&registry)? {
                        match atalaya_core::radar::check(&registry, c.id) {
                            Ok(true) => println!("{:<22} cambió", c.name),
                            Ok(false) => println!("{:<22} sin cambios", c.name),
                            Err(e) => println!("{:<22} error: {e:#}", c.name),
                        }
                    }
                }
                RadarCmd::Remove { id } => {
                    atalaya_core::radar::remove(&registry, id)?;
                    println!("Página {id} quitada del radar.");
                }
            }
        }
        Cmd::Backup { slug } => {
            let registry = Registry::open_default()?;
            for project in registry.list()? {
                if slug.as_ref().is_some_and(|s| *s != project.slug) {
                    continue;
                }
                match atalaya_core::backup::backup_project(&registry, &project)? {
                    Some((c, _)) => {
                        println!("{:<12} {:<6} {}", c.slug, c.status, c.summary);
                        if let Some(e) = c.details["error"].as_str() {
                            println!("             {e}");
                        }
                    }
                    None => println!("{:<12} sin base de datos conocida", project.slug),
                }
            }
            println!("Carpeta: {}", atalaya_core::backup::root()?.display());
        }
        Cmd::Conversaciones { action } => {
            use atalaya_core::conversations;
            let registry = Registry::open_default()?;
            match action {
                ConversacionesCmd::List { limite } => {
                    let all = conversations::list(&registry, limite)?;
                    if all.is_empty() {
                        println!("Todavía no hay conversaciones guardadas.");
                    }
                    for c in all {
                        println!("{}  {}  {:>3} mensajes  {}", &c.id[..8.min(c.id.len())], c.updated_at, c.messages, c.title);
                    }
                }
                ConversacionesCmd::Buscar { texto, project } => {
                    let hits = conversations::search(&registry, &texto, project.as_deref(), 30)?;
                    if hits.is_empty() {
                        println!("No aparece «{texto}» en ninguna conversación.");
                    }
                    for h in hits {
                        println!("{}  {}  {}
    {}", &h.summary.id[..8.min(h.summary.id.len())], h.summary.updated_at, h.summary.title, h.snippet);
                    }
                }
                ConversacionesCmd::Ver { id } => print!("{}", conversations::markdown(&registry, &id)?),
                ConversacionesCmd::Exportar { id, salida } => match salida {
                    None => print!("{}", conversations::markdown(&registry, &id)?),
                    Some(path) => {
                        if path.exists() {
                            bail!("{} ya existe; elige otro nombre", path.display());
                        }
                        conversations::export(&registry, &id, &path)?;
                        println!("Exportada a {}", path.display());
                    }
                },
            }
        }
        Cmd::Secretos { carpeta, desde } => {
            let found = atalaya_core::leaks::scan_repo(&carpeta, &desde, true)?;
            if found.is_empty() {
                println!("Sin secretos a la vista desde {desde} (incluye lo que aún no confirmas).");
            } else {
                println!("{}", atalaya_core::leaks::report(&found));
                bail!("{} posibles secretos; si alguno es a propósito, marca su línea con atalaya:permitir", found.len());
            }
        }
        Cmd::Telegram { action } => {
            let registry = Registry::open_default()?;
            match action {
                TelegramCmd::Setup { chat } => {
                    registry.set_setting("telegram_chat", &chat)?;
                    if secrets::get("telegram").is_none() {
                        ask_secret("telegram", "la llave del bot que te dio @BotFather")?;
                    }
                    println!("Telegram configurado. Pruébalo con: atalaya telegram test");
                }
                TelegramCmd::Test => {
                    if !checks::telegram(&registry, "Mensaje de prueba: los avisos graves llegarán aquí.") {
                        bail!("no se pudo enviar; revisa la llave del bot y el chat, y que le hayas escrito al bot al menos una vez");
                    }
                    println!("Mensaje de prueba enviado.");
                }
                TelegramCmd::Decisiones { valor } => {
                    match valor.as_deref() {
                        Some("si" | "sí") => atalaya_core::telegram::set_enabled(&registry, true)?,
                        Some("no") => atalaya_core::telegram::set_enabled(&registry, false)?,
                        Some(other) => bail!("usa si o no, no «{other}»"),
                        None => {}
                    }
                    let status = atalaya_core::telegram::status(&registry)?;
                    match (status["configured"].as_bool(), status["decisions"].as_bool()) {
                        (Some(false), _) => println!("Telegram no está configurado: atalaya telegram setup <chat>"),
                        (_, Some(true)) => println!("Las decisiones nuevas llegan a Telegram con botones."),
                        _ => println!("Las decisiones no llegan a Telegram (los avisos graves sí)."),
                    }
                }
                TelegramCmd::Escuchar => {
                    println!("Escuchando Telegram. Ctrl+C para detener.");
                    loop {
                        if !atalaya_core::telegram::listen(&registry, 50)? {
                            bail!("Telegram no está listo: falta el chat, la llave del bot, o las decisiones están apagadas");
                        }
                    }
                }
            }
        }
        Cmd::Firebase { action } => match action {
            FirebaseCmd::Connect { file } => {
                let email = atalaya_core::firebase::connect(&file)?;
                println!("Cuenta de servicio guardada en el Llavero: {email}");
                println!("Ya puedes borrar el archivo: {}", file.display());
            }
        },
        Cmd::SupabaseProjects => {
            for (name, reference, org, status) in checks::supabase_projects()? {
                println!("{reference}  {org}  {status:<16} {name}");
            }
        }
        Cmd::Check { slug, notify } => {
            let registry = Registry::open_default()?;
            let results = checks::run_due(&registry, slug.as_deref(), true)?;
            for (c, _) in &results {
                println!("{:<12} {:<13} {:<6} {}", c.slug, c.kind, c.status, c.summary);
            }
            if notify {
                checks::notify_changes(&registry, &results)?;
            }
        }
        Cmd::Work { id } => {
            let registry = Registry::open_default()?;
            let done = atalaya_core::worker::run(&registry, id, &|kind, text| {
                let first = text.lines().next().unwrap_or_default();
                match kind {
                    "estado" => println!("== {first}"),
                    "paso" => println!("   {first}"),
                    _ => {}
                }
            })?;
            println!("\nEstado: {}", done.status);
            if let Some(url) = done.pr_url {
                println!("Pull request: {url}");
            }
            if let Some(result) = done.result {
                println!("\n{result}");
            }
        }
        Cmd::Service { action } => match action {
            ServiceCmd::Install => {
                status::install_service()?;
                println!("Vigía instalado y en guardia. Arrancará solo en cada inicio de sesión.");
            }
            ServiceCmd::Uninstall => {
                status::uninstall_service()?;
                println!("Vigía detenido y quitado.");
            }
            ServiceCmd::Status => match status::service_running() {
                Some(true) => println!("En guardia."),
                Some(false) => println!("Detenido. Instálalo con: atalaya service install"),
                None => println!("No se puede consultar en este sistema."),
            },
        },
        Cmd::Watch => {
            println!("Atalaya vigilando. Ctrl+C para detener.");
            // Los botones de Telegram se escuchan aparte, con espera larga, para responder al instante.
            std::thread::spawn(|| loop {
                match Registry::open_default().and_then(|r| atalaya_core::telegram::listen(&r, 50)) {
                    Ok(true) => {}
                    Ok(false) => std::thread::sleep(std::time::Duration::from_secs(60)),
                    Err(e) => {
                        eprintln!("telegram: {e:#}");
                        std::thread::sleep(std::time::Duration::from_secs(30));
                    }
                }
            });
            loop {
                // Un error puntual (red caída, base ocupada) no debe tumbar al vigía.
                let round = Registry::open_default().and_then(|registry| {
                    let results = checks::run_due(&registry, None, false)?;
                    checks::notify_changes(&registry, &results)?;
                    // Una falla nueva: la IA de la reacción la diagnostica en otro hilo, sin atrasar la vuelta.
                    if let Err(e) = atalaya_core::automations::react(&registry, &results, true) {
                        eprintln!("no se pudo reaccionar a las fallas: {e:#}");
                    }
                    match atalaya_core::automations::round(&registry) {
                        Ok(0) => {}
                        Ok(n) => println!("{n} tareas programadas corridas."),
                        Err(e) => eprintln!("no se pudieron correr las tareas programadas: {e:#}"),
                    }
                    // Un correo que no sale no debe hacer fallar la ronda.
                    // Radar y ronda del agente, una vez al día cada uno.
                    if let Err(e) = atalaya_core::radar::round(&registry) {
                        eprintln!("no se pudo revisar el radar: {e:#}");
                    }
                    match atalaya_core::round::daily(&registry) {
                        Ok(true) => println!("Ronda del agente hecha."),
                        Ok(false) => {}
                        Err(e) => eprintln!("la ronda del agente falló: {e:#}"),
                    }
                    // Respaldo diario de las bases; si falla o se recupera, avisa como cualquier revisión.
                    match atalaya_core::backup::round(&registry) {
                        Ok(backups) => checks::notify_changes(&registry, &backups)?,
                        Err(e) => eprintln!("no se pudieron hacer los respaldos: {e:#}"),
                    }
                    if let Err(e) = atalaya_core::claude::round(&registry) {
                        eprintln!("no se pudo leer Claude Platform: {e:#}");
                    }
                    // Con la autonomía activada, lo que pasó sus comprobaciones se fusiona solo.
                    if let Err(e) = atalaya_core::worker::auto_merge(&registry) {
                        eprintln!("no se pudo fusionar automáticamente: {e:#}");
                    }
                    // Los pull request que ya se fusionaron en GitHub cierran su mejora.
                    if let Err(e) = atalaya_core::worker::sync_merged(&registry) {
                        eprintln!("no se pudo revisar los pull request: {e:#}");
                    }
                    // Las carpetas locales que quedaron atrasadas se reintentan en cada vuelta.
                    if let Err(e) = atalaya_core::local_sync::retry(&registry) {
                        eprintln!("no se pudo poner al día las carpetas locales: {e:#}");
                    }
                    match mail::daily_report(&registry) {
                        Ok(true) => println!("Resumen diario enviado por correo."),
                        Ok(false) => {}
                        Err(e) => eprintln!("no se pudo enviar el resumen diario: {e:#}"),
                    }
                    Ok(results)
                });
                match round {
                    Ok(results) => {
                        for (c, prev) in results {
                            if prev.as_deref() != Some(c.status.as_str()) {
                                println!("{} {}: {} — {}", c.slug, c.kind, c.status, c.summary);
                            }
                        }
                    }
                    Err(e) => eprintln!("error en la ronda: {e:#}"),
                }
                std::thread::sleep(std::time::Duration::from_secs(60));
            }
        }
        Cmd::Report { email } => {
            let registry = Registry::open_default()?;
            let text = checks::report(&registry)?;
            print!("{text}");
            if email {
                let (today, _) = registry.local_now()?;
                mail::send(&registry, &format!("Atalaya: reporte del {today}"), &text)?;
                println!("\nReporte enviado por correo.");
            }
        }
        Cmd::Mcp { read_only } => tokio::runtime::Runtime::new()?.block_on(mcp::serve(read_only))?,
        Cmd::McpPuente { file } => atalaya_core::mcp_bridge::relay(&file)?,
    }
    Ok(())
}

