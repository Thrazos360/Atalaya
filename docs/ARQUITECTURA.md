# Arquitectura y desarrollo

Guía para compilar Atalaya, entender cómo está armada y agregarle proveedores. Si solo
quieres usarla, mira [INSTALAR.md](../INSTALAR.md).

## Arquitectura

```
crates/
  atalaya-core/   la lógica, sin interfaz
  atalaya/        el comando `atalaya`: terminal, vigía en segundo plano y servidor MCP
app/
  src-tauri/      la app de escritorio (Tauri 2): comandos que llaman al núcleo
  src/            la interfaz (React + TypeScript + Vite)
scripts/          versión, firma local y paquete .pkg
```

**El núcleo** (`crates/atalaya-core/src`):

| Módulo | Qué hace |
|---|---|
| `registry.rs` | La base SQLite: proyectos, revisiones, mejoras, eventos y ajustes. Las migraciones solo agregan. |
| `secrets.rs` | Las llaves, en el Llavero de macOS. Nunca van a la base. |
| `connections.rs` | Conexiones genéricas (varias por proveedor) y vínculos por proyecto (repo, hosting, base, respaldo). |
| `providers/` | Cómo se prueba y se revisa cada proveedor: `hosting`, `database` y `forge` (GitLab y Gitea). |
| `checks.rs` | Las revisiones periódicas (`KINDS`) y sus avisos. `notify` envía la notificación a nombre de la app (`ATALAYA_APP_ID`) y anota qué vista abre al hacer clic. |
| `discover.rs` | Carpetas de proyectos, escaneo y registro. |
| `backup.rs` | Respaldos de Supabase, Firestore, Postgres, MySQL y SQLite. |
| `ai.rs` | Cliente único para tres formatos (Anthropic, OpenAI y compatibles, Gemini), más el registro de consumo, los precios, los presupuestos y el tope por encargo (`fit_cap` acorta la respuesta de un modelo con precio conocido; `claude_cap_args` le pasa `--max-budget-usd` a Claude Code). Todo lo que lanza Claude Code pasa por `tracked_claude`, para que quede anotado como las demás IA. |
| `agents.rs` | Agentes de terminal (Codex, Antigravity, Cursor, Kimi Code o uno propio) dentro de `sandbox-exec`. `OWN_MODELS`: los que pueden usar un modelo de este Mac o de un servicio en vez de su cuenta (hoy Qwen Code). |
| `clis.rs` | El puente con los agentes que usan tu cuenta: cómo se instala cada uno, cómo inicia sesión, cómo saber si la tiene, cómo pedirle una respuesta en solo lectura y cómo comprobar que responde (`verify`). |
| `local.rs` | Encuentra los servidores de modelos de este Mac (Ollama, LM Studio, Jan, llama.cpp), sus modelos (los que sirven y los descargados) y enciende el servidor cuando la app trae su comando. |
| `orchestrator.rs` | Roles (constructor, revisor, validador, asistente), estrategias y paso al siguiente candidato. |
| `context.rs` | El paquete de contexto común que recibe cualquier modelo. |
| `worker.rs` | Lleva una mejora aceptada hasta un pull request, con revisión cruzada. Si un constructor termina sin commits pasa al siguiente, salvo que concluya «Sin cambios: …» (no hacía falta tocar el código): entonces la mejora queda hecha sin pull request; si ninguno lo logra, notifica con la vista `agente:<id>` para responder en el Agente. La respuesta del usuario (`feedback` de la mejora) va en el siguiente intento. |
| `delegate.rs` | «Claude Code coordina»: con la opción encendida, Claude Code en el chat le pide consultas y cambios a las demás IA de tus roles (`delegar_consulta` y `delegar_trabajo` del MCP), o varias a la vez en paralelo, repartidas entre el rol (`delegar_consultas` y `delegar_trabajos`). Solo a las que están en tus roles; los agentes trabajan aislados en la carpeta del proyecto. |
| `round.rs` | La ronda diaria del validador. |
| `decisions.rs` | La cola de Decisiones: nada se ejecuta sin aprobación. |
| `activity.rs` | Lo que hacen en vivo las IA delegadas desde el chat: quién empezó, cada línea que escriben y cómo terminaron. El servidor MCP lo anota y la vista Agente lo lee mientras trabajan. |
| `approvals.rs` | Las aprobaciones del chat: la herramienta MCP `aprobar_accion` deja aquí lo que el agente quiere hacer y espera la respuesta de la app. |
| `attachments.rs` | Los adjuntos del chat (`adjuntos/<conversación>/` junto a la base). Claude Code los lee desde su ruta con `--add-dir`; los agentes, desde su encierro (`Policy.readable` abre solo esa carpeta, para leer); los modelos por API los reciben como imágenes dentro del mensaje (`Message.images`) y el texto de los archivos de texto. |
| `mcp_bridge.rs` | El MCP de Atalaya para los agentes de terminal, que corren encerrados sin leer la base. `agents::spawn` abre en cada ejecución un puerto en 127.0.0.1 con contraseña de un solo uso; el agente lanza `atalaya mcp-puente <archivo>` (solo pasa bytes) y Atalaya atiende cada conexión con `atalaya mcp --read-only` fuera del encierro (`READ_ONLY` en `crates/atalaya/src/mcp.rs`: una herramienta nueva queda fuera hasta agregarla). Codex lo recibe con `-c mcp_servers…`; Gemini CLI, con `.gemini/settings.json` en su carpeta temporal (solo fuera de todo repositorio); los demás, en `{mcp_config}` o `ATALAYA_MCP_CONFIG`. |
| `mcp_servers.rs` | Servidores MCP externos compartidos (programas o direcciones web; se agregan apagados y se prueban con `test`). `for_run` entrega los que rigen para un proyecto y una IA; la llave sale del Llavero solo hacia el entorno de esa ejecución (`ATALAYA_MCP_<NOMBRE>`). Claude Code los recibe en su `--mcp-config` (`round::mcp_config_plus`); los agentes de terminal, por `Policy.mcp` y `mcp_bridge`. |
| `memories.rs` | La memoria de los proyectos: una IA la propone (`proponer_memoria` del MCP), el usuario la aprueba, ajusta o descarta, y lo aprobado llega a todas las IA con las instrucciones (`skills::for_ai`). Nada se borra: lo olvidado queda `descartada`. |
| `commands.rs` | Comandos guardados que se llaman en el chat con `/nombre`; `expand` arma el mensaje con lo escrito después (`{texto}`). |
| `compare.rs` | La misma pregunta a varias IA a la vez (de 2 a 4), cada una en su hilo; avisa cada respuesta apenas llega y anota el consumo como «comparar». Solo leen. |
| `conversations.rs` | Historial del chat: la app guarda cada conversación tras cada turno (por posición, así que guardar de nuevo actualiza y agrega), y se puede listar, buscar (sin distinguir mayúsculas, con trozo alrededor), retomar con `recap` y exportar a Markdown. |
| `quota.rs` | IA sin cuota: reconoce el aviso (cuota, límite de uso, 429) en el error de una llamada, lee cuándo se renueva y la deja en pausa en `sin_cuota`; el orquestador la salta y una llamada buena la quita. También elige la línea que explica el fallo de un agente. |
| `local_sync.rs` | Pone al día la carpeta local con la rama principal tras fusionar: solo fast-forward y deja que git decida qué es seguro (lo que no choca se conserva). Si queda bloqueada, anota el proyecto en `local_pendiente`, el Vigía lo reintenta en cada vuelta y deja una decisión `actualizar` que ejecuta Atalaya (no un agente); al resolverse, la cierra sola. |
| `leaks.rs` | Revisión de secretos: llaves y tokens conocidos, JWT `service_role` de Supabase, contraseñas escritas en el código y archivos sensibles (`.env`, llaves privadas, cuentas de servicio). El constructor la corre antes de subir cada rama; nunca muestra el secreto completo. `atalaya:permitir` en una línea la salta. |
| `telegram.rs` | Decisiones desde el teléfono: manda cada decisión nueva con botones, escucha las respuestas con `getUpdates` (espera larga, sin webhooks) desde la Vigía y solo obedece al chat configurado. `handle` no toca la red y se prueba solo; lo enviado queda en `telegram_sent`. |
| `automations.rs` | Tareas programadas (cada día, semana u horas; `round` las corre desde la Vigía) y reacciones a las fallas nuevas de las revisiones (`react`, en otro hilo, sin repetir antes de 6 horas). Solo leen; avisan por el Mac y Telegram y dejan historial en `automation_runs`. |
| `skills.rs` | Skills (carpetas con `SKILL.md`, en `skills/` junto a la base; se instalan apagadas) e instrucciones comunes (global y por proyecto). `for_ai` las entrega según quién lee: Claude Code con herramientas recibe las skills como plugin de la sesión (`--plugin-dir`); los agentes de terminal, la ruta de cada `SKILL.md` (`Policy.readable` abre la carpeta solo para leer); lo que no lee archivos, su texto. Se usan en el chat, las mejoras (`worker.rs`), los encargos delegados, la revisión y la ronda. |

**La vista Agente** (`app/src-tauri/src/chat.rs`) lanza Claude Code con
`--permission-prompt-tool mcp__atalaya__aprobar_accion` y su propio `--mcp-config`.
La base hace de puente con la app, porque el servidor MCP es otro proceso. Los modos se
traducen en opciones de Claude Code:

- `preguntar` usa `--permission-mode default`;
- `autonomo` usa `bypassPermissions`;
- `lectura` usa una lista cerrada de herramientas.

Los controles de Mejoras y Decisiones dentro del chat están en
`app/src/views/ChatBoard.tsx`: la barra «Tablero» y la tarjeta con botones que sale bajo
cada paso que toca una mejora o una decisión. Usan los mismos comandos que esas vistas.
Claude Code puede resolver decisiones con `resolve_decision` del MCP, siempre con tu
visto bueno.

## Compilar

Necesitas:

- Rust estable (con `rustup`);
- Node 20 o más nuevo, con npm;
- las herramientas de línea de comandos de Xcode.

```bash
cargo test --workspace
```

```bash
cd app && npm install && npm run build
```

El paquete instalador queda en `dist/`:

```bash
scripts/build-pkg.sh
```

Sin la firma local (`scripts/crear-firma.sh`), macOS vuelve a pedir permisos con cada
versión nueva.

## Probar sin tocar una instalación real

Hay tres cosas que se pueden separar de la instalación real:

- `ATALAYA_DB` usa otra base;
- `ATALAYA_KEYCHAIN_SERVICE` usa otro servicio del Llavero;
- otro identificador evita chocar con la app instalada, que solo admite una instancia.

```bash
ATALAYA_DB=/tmp/atalaya-prueba.db ATALAYA_KEYCHAIN_SERVICE=Atalaya-prueba cargo run -p atalaya -- doctor
```

```bash
cd app && npx tauri build --debug --bundles app --config '{"identifier":"cl.thrazos.atalaya.prueba","productName":"Atalaya Prueba"}'
```

```bash
ATALAYA_DB=/tmp/atalaya-prueba.db ATALAYA_KEYCHAIN_SERVICE=Atalaya-prueba "target/debug/bundle/macos/Atalaya Prueba.app/Contents/MacOS/atalaya-app"
```

## Agregar un proveedor

1. **`connections.rs`**: agrega una entrada a `PROVIDERS` con:
   - `id` y `kind` (`git`, `hosting`, `base`, `avisos`, `ia` o `agente`);
   - los campos que pide;
   - si necesita llave y dónde se crea.

   La vista Conexiones arma su formulario sola.
2. **`providers/mod.rs`**: en `test`, cómo se prueba. Para las respuestas usa
   `providers::http`, `expect_2xx` y `combine`.
3. **`checks.rs`**: si se revisa en cada vuelta, agrega una revisión a `KINDS` y su caso
   en `run_one`. Lee el vínculo del proyecto con `connections::linked`.
4. **Si sirve para respaldar**: agrega el caso en `backup.rs`.

Para una IA:

- si habla uno de los tres formatos de `ai.rs`, basta el paso 1 y agregar su id en
  `format_of`;
- un agente de terminal necesita su proveedor en `connections.rs` (con `command` para
  trabajar y `ask_command` para responder, ambos con `{tarea}`) y su entrada en
  `clis::CATALOG`: programa, comando de instalación, comando de inicio de sesión, cómo
  saber si tiene la sesión (preguntándole al programa o por la existencia de un
  archivo, sin leerlo) y las carpetas donde guarda su sesión;
- un servicio con la API de OpenAI solo necesita una línea en `API_PRESETS`;
- lo gratis va primero en toda la app: marca el uso gratis en `free` (agentes en
  `clis::CATALOG`, servicios en `API_PRESETS`, proveedores en `FREE_PROVIDERS`). Cuenta
  solo un uso gratis que se renueva, no créditos de regalo, y revisa las fuentes: cambian
  seguido;
- un agente que acepta un modelo con la API de OpenAI por variables de entorno, una
  entrada en `agents::OWN_MODELS`;
- un servidor local, una entrada en `local::SERVERS`.

Cada pieza nueva lleva su prueba. Para no depender de la red, usa los servidores falsos
de `testutil.rs` y de las pruebas de `ai.rs`.

## Seguridad

- Las llaves solo viven en el Llavero. La interfaz nunca las recibe de vuelta.
- Quitar una conexión no borra su llave, salvo que se pida.
- Los agentes trabajan en una copia aparte (un `git worktree`) y en una rama propia:
  - Claude Code usa su propio aislamiento;
  - los demás corren en `sandbox-exec`: escriben solo en su copia, sus cachés y su rama,
    no ven `~/.ssh`, `~/.aws`, credenciales de git ni los datos de Atalaya, y tienen
    bloqueado el `git push`. Pueden pedir al Llavero su propia sesión: macOS solo les
    entrega lo que guardó cada uno y pregunta por lo demás. No se bloquea porque macOS
    usa el mismo servicio para validar los certificados HTTPS, y sin él Codex,
    Antigravity y Cursor no funcionan.
- Las versiones antiguas que comparten nombre con un agente y ya no funcionan (como el
  kimi-cli de Python) se ignoran: `legacy` en `clis::CATALOG`.
- Atalaya sube la rama y abre el pull request. Fusionarlo es decisión de una persona.
- El revisor nunca es el mismo modelo que construyó.
- Atalaya nunca lee las credenciales de los agentes. Para saber si tienen la sesión
  iniciada, les pregunta a ellos o mira si existe su archivo de sesión. Iniciar sesión
  e instalar se hacen en la Terminal, a la vista.
- Cuando un agente solo responde (revisor, validador, chat en solo lectura), las
  carpetas de los proyectos quedan protegidas contra escritura dentro del encierro.
- En Agente, el modo normal pide aprobación para cada acción que cambia algo. Una
  aprobación vale solo para su conversación, y lo que nadie responde en 15 minutos se
  rechaza.

## Versiones

- La versión está en el `Cargo.toml` de la raíz.
- `scripts/version.sh <nueva>` la cambia, crea el commit y la etiqueta.
- Los cambios de cada versión están en [CHANGELOG.md](../CHANGELOG.md).

## Publicar en el repo público

Quien mantiene el proyecto trabaja en un repo aparte y publica con
`scripts/publicar.sh`:

1. Copia lo confirmado, sin el historial, a un clon del repo público.
2. Antes de confirmar, revisa:
   - términos de una lista local (`.publicar-prohibido`, que nunca va a git), también
     dentro de las imágenes;
   - llaves, tokens y archivos sensibles;
   - `atalaya secretos`.
3. Se detiene si en GitHub hay pull requests fusionados que aún no trajiste, para no
   borrarlos.
4. Los commits públicos llevan hora UTC.

`scripts/publicar.sh --revisar` muestra qué cambiaría sin tocar nada, y `--help` lista
las opciones.
