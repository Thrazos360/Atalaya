# Historial de versiones

Atalaya usa versiones `MAYOR.MENOR.PARCHE`. Mientras esté en `0.x`:

- **MENOR** (0.6 → 0.7): funciones nuevas o cambios visibles.
- **PARCHE** (0.6.0 → 0.6.1): correcciones sin funciones nuevas.
- **1.0.0** queda para cuando Atalaya corra estable en el Mac y en Proxmox.

La versión se define en un solo lugar (`Cargo.toml` de la raíz) y se cambia con
`scripts/version.sh <nueva>`, que además crea el commit y la etiqueta `v<nueva>`.

## 0.42.0 — 2026-11-25

- **Más servicios de IA gratis listos para conectar.** En Conexiones → IA → Compatible con
  OpenAI se suman **SambaNova** (plan Free sin tarjeta) y **Cloudflare Workers AI** (10 000
  «neuronas» al día). Para Cloudflare, Atalaya indica dónde poner el ID de tu cuenta, no deja
  guardar la dirección sin él y «Probar» lista sus modelos de texto.
- **NVIDIA NIM cuenta como gratis**: su uso de prueba no cobra, solo limita los pedidos por
  minuto. Los roles con la estrategia «gratis primero» ya lo ponen adelante.
- **GitHub Models sale de la lista**: GitHub lo retiró el 30 de julio de 2026 y su API ya no
  responde. **Cerebras** sigue en la lista, pero no como gratis: hoy solo da US$ 5 de prueba
  por 30 días y pide tarjeta.

## 0.41.0 — 2026-11-25

- **Una IA sin cuota queda en pausa en vez de fallar una y otra vez.** Cuando un agente o
  un modelo responde que se le acabó la cuota (el plan gratis de Cursor, la cuota semanal
  de Antigravity, un 429 de un servicio…), Atalaya lo salta hasta que se renueve y le pasa
  el trabajo al siguiente de la lista. Si el aviso dice cuándo («Resets in 128h»), espera
  hasta entonces; si no, vuelve a probar en unas horas. Una llamada que sale bien la saca
  de la pausa.
- **Consumo dice por qué falló cada IA.** Antes solo se veía «14 con error». Ahora queda
  anotada la línea que explica el fallo, se muestra el último error y, si está sin cuota,
  hasta cuándo, con un «probar ya» para quitar la pausa a mano. En Ajustes → IA el rol
  también muestra «sin cuota hasta el…», y la Actividad avisa la primera vez.

## 0.40.0 — 2026-11-24

- **La carpeta local se pone al día sola cuando es seguro.** Antes, cualquier archivo sin
  confirmar frenaba la actualización tras fusionar, aunque fuera una caché que el mismo
  cambio pasaba a ignorar. Ahora Atalaya avanza y deja que git decida: lo que tienes sin
  confirmar y no choca con el cambio se queda como estaba; si algo se pisaría, no toca nada.
- **Si no puede, lo reintenta y lo hace Atalaya, no un agente.** El Vigía vuelve a intentarlo
  en cada vuelta. Mientras tanto queda en Decisiones un «Poner al día» (también con botón
  en Telegram) que explica qué lo impide y se cierra solo cuando se resuelve. La ronda ya no
  propone esto como mejora: los agentes trabajan en una copia aparte y no pueden tocar tu
  carpeta.
- **Un agente que concluye que no hay nada que cambiar cierra la mejora.** Si termina con
  «Sin cambios: …», la mejora queda hecha con su explicación, sin pull request y sin
  pasársela al siguiente agente, que gastaría otro intento para llegar a lo mismo.
- **Avisos de git más claros.** «1 archivo sin confirmar: supabase/.temp/ (nuevo)» en vez de
  «1 archivos sin confirmar»: con nombre y tipo (nuevo, modificado, borrado), y en singular
  cuando corresponde.

## 0.39.0 — 2026-11-24

- **Historial del chat.** Cada conversación del Agente queda guardada. Con **Historial**
  buscas en todas (por ejemplo «SUPABASE_URL»), abres una y sigues desde ahí (la IA recibe
  lo conversado) o la exportas a Markdown. Terminal:
  `atalaya conversaciones list|buscar|ver|exportar`.
- **Revisión de secretos antes de cada pull request.** Antes de subir la rama, el
  constructor revisa lo que agrega el cambio:
  - llaves y tokens conocidos (Anthropic, OpenAI, GitHub, GitLab, AWS, Google, Stripe,
    Slack, Supabase, Telegram);
  - la llave `service_role` de Supabase (la `anon` no cuenta: es pública a propósito);
  - direcciones de bases con contraseña y contraseñas escritas en el código;
  - archivos como `.env`, llaves privadas y cuentas de servicio.
- Si encuentra algo, la rama no sale del Mac: la mejora queda en revisión, sin pull request,
  explicando dónde está. Avisa por el Mac y por Telegram. Nunca muestra el secreto
  completo, solo sus primeros caracteres.
- Para un falso positivo, marca la línea con `atalaya:permitir`.
- `atalaya secretos [carpeta] [--desde ref]` hace la misma revisión a mano, incluido lo que
  aún no confirmas.
- La prueba de un agente personalizado corre fuera de cualquier proyecto: un script que no
  entendiera `--version` ya no puede ponerse a trabajar en la carpeta donde estás.
- Los avisos por Telegram ya no consultan el Llavero si Telegram no está configurado.

## 0.38.0 — 2026-11-24

- **Decisiones desde Telegram.** Cada decisión nueva llega al teléfono con botones:
  **Aprobar** (o **Que la trabaje el agente** / **Solo vista** en las informativas) y
  **Descartar**. Al aprobar una que crea mejora, el agente la empieza y avisa por Telegram
  cómo terminó, con el enlace al pull request.
- Solo obedece al chat configurado: lo que llegue de otro chat se ignora.
- Lo que se decide en la app también se refleja en Telegram: el mensaje pierde sus botones
  y dice cómo terminó.
- Comandos al bot: `/decisiones` (las pendientes, con botones), `/estado` y `/ayuda`.
- Las decisiones que ya esperaban antes de activarlo no llegan de golpe; se piden con
  `/decisiones`. Se manda un máximo de 5 nuevas por vuelta.
- Lo escucha la Vigía en un hilo aparte, con espera larga: responde al instante, sin
  abrir puertos. Se apaga desde Decisiones o con `atalaya telegram decisiones no`.
- Terminal: `atalaya telegram decisiones [si|no]` y `atalaya telegram escuchar`.

## 0.37.0 — 2026-11-24

- **Tareas programadas** (vista Tareas, nueva). Un encargo que corre solo cada día, cada
  semana o cada tantas horas, con la IA que elijas: «resume la semana de Faro», «revisa si
  hay dependencias con fallas», o un comando guardado como `/resumen-semana faro`. Una
  tarea nueva espera a su próximo horario; también se puede correr ahora.
- **Reacciones a las fallas.** Cuando una revisión pasa a falla (un despliegue, el sitio,
  GitHub, la base…), la IA que elijas diagnostica la causa apenas la Vigía lo detecta, y si
  quieres deja el diagnóstico como mejora propuesta. Para el mismo proyecto y revisión no
  se repite antes de 6 horas. Corre en otro hilo: no atrasa las revisiones.
- Las dos solo leen. Lo que encuentran llega como aviso del Mac (y por Telegram si está
  configurado) y queda en el historial de la vista.
- Las corre la Vigía (`atalaya watch`): con la Vigía detenida, la vista lo avisa.
- Terminal: `atalaya tareas list|add|run|pause|resume|remove` y
  `atalaya reacciones list|add|remove`.

## 0.36.0 — 2026-11-24

- **Preguntar a varias IA a la vez.** En el chat, **Comparar** manda la misma pregunta a
  dos, tres o cuatro IA (Claude Code, agentes de terminal o modelos por API) y muestra sus
  respuestas lado a lado, cada una apenas llega, con lo que tardó y lo que costó. Solo
  leen. Con **Seguir con…** continúas la conversación con la que más te sirvió, y le llega
  lo que ya respondió.
- **Tope de gasto por encargo** (Consumo → Tope por encargo). Lo máximo que puede gastar un
  solo encargo: un turno del chat, una mejora, una consulta o la ronda.
  - Claude Code se detiene al llegar (`--max-budget-usd`) y el chat dice por qué. Lo revisa
    entre un paso y otro, así que un paso largo puede pasarlo un poco.
  - Los modelos por API con precio conocido acortan su respuesta para no pasarlo, y si solo
    leer ya costaría más, no llaman. Sin precio conocido no se limitan: no se inventan
    precios.
  - Los agentes de terminal no informan su gasto mientras trabajan: a ellos los limita el
    tiempo.
- Corregido: dos encargos de un reparto que empezaban en el mismo instante podían mezclarse
  en un solo recuadro del chat.
- Terminal: `atalaya comparar "pregunta" --ia uno --ia otro [--project slug]` y
  `atalaya ai tope [dólares]`.

## 0.35.0 — 2026-11-24

- **Memoria por proyecto.** Lo que vale la pena recordar de una conversación («este repo
  usa pnpm», «la tabla pagos no se toca sin respaldo») queda guardado. Claude Code lo
  propone desde el chat con una tarjeta (**Aprobar**, **Ajustar**, **Descartar**), y lo
  aprobado llega a todas las IA junto a las instrucciones, en cada encargo.
- La vista **Skills, memoria y MCP** muestra lo propuesto y lo aprobado. También puedes
  escribir una memoria tú, para un proyecto o para todos, editarla u olvidarla. Lo que se
  olvida queda descartado, no se borra.
- **Comandos guardados.** Recetas que llamas en el chat con `/nombre` y lo que quieras
  después; sirven con cualquier IA. Al escribir `/` el chat sugiere los tuyos (Tab o Enter
  completa). Trae cuatro de ejemplo: `/revisar-seguridad`, `/subir-dependencias`,
  `/explicar` y `/resumen-semana`.
- MCP de Atalaya: `proponer_memoria` y `list_memories` (este también para los agentes, en
  solo lectura).
- Terminal: `atalaya memoria list|add|approve|discard` y `atalaya comandos
  list|add|examples|remove`.

## 0.34.0 — 2026-11-24

- **Servidores MCP compartidos.** En la vista **Skills y MCP** agregas una vez un servidor
  MCP (Supabase, GitHub, Context7, Playwright o el tuyo, sea un programa o una dirección
  web) y Atalaya se lo entrega a Claude Code en el chat y a los agentes de terminal cuando
  trabajan. A Gemini CLI le llega en las consultas.
- **Se agregan apagados.** Con **Probar** Atalaya lo arranca y lista sus herramientas, y
  solo entonces se puede activar. Puede regir en todos los proyectos o en algunos, y para
  todas las IA o algunas. Los programas corren en el mismo aislamiento que los agentes.
- **La llave nunca queda a la vista.** Va al Llavero y viaja solo en el entorno de esa
  sesión: Claude Code y Gemini la leen como `${VARIABLE}`, y Codex con `env_vars` o
  `bearer_token_env_var`. No va en la base ni en ninguna línea de comando.
- En el chat, el paso dice de qué servidor es cada herramienta («context7 ·
  resolve-library-id»).
- Terminal: `atalaya mcps list|add|test|enable|disable|remove`.

## 0.33.0 — 2026-11-24

- **El MCP de Atalaya también para los agentes de terminal.** Codex, Gemini CLI y los
  agentes propios consultan Atalaya con las mismas herramientas que Claude Code
  (proyectos, estado, commits, mejoras, decisiones, consumo, roles, issues, radar), pero
  solo las que leen: no pueden cambiar interruptores, aprobar decisiones ni delegar.
- **Sin abrirles los datos.** El agente sigue encerrado sin poder leer la carpeta de
  Atalaya. En cada ejecución Atalaya abre un puerto en 127.0.0.1 con una contraseña de un
  solo uso (en un archivo que el agente lee pero no puede cambiar, nunca en la línea de
  comando), y atiende cada consulta con `atalaya mcp --read-only` fuera del encierro.
  Al terminar el agente se cierra todo.
- **Sin tocar su configuración de siempre.** Codex lo recibe con `-c mcp_servers…` en esa
  ejecución. Gemini CLI, con `.gemini/settings.json` en su carpeta temporal, solo en las
  consultas y nunca dentro de un proyecto o un repositorio; se quita al terminar. Un
  agente propio lo recibe en `{mcp_config}` de su plantilla o en la variable
  `ATALAYA_MCP_CONFIG` (un JSON `mcpServers`).
- Terminal: `atalaya mcp --read-only`.

## 0.32.0 — 2026-11-24

- **Skills para todas las IA.** Nueva vista **Skills**: instalas una skill (una carpeta con
  su `SKILL.md`) desde una carpeta, un repo de git o importándola de las que ya tienes en
  Claude Code o Codex, y Atalaya se la entrega a cada IA cuando trabaja:
  - Claude Code la recibe como plugin de la sesión y la usa como propia;
  - los agentes de terminal leen su `SKILL.md` desde su encierro, solo para leer;
  - los modelos por API (y Claude Code sin herramientas, en la ronda o la revisión)
    reciben su texto, hasta un tope.
- **Se instalan apagadas.** Antes de activarla ves sus archivos y su texto, con aviso si
  trae programas que una IA podría ejecutar. Cada skill puede regir en todos los
  proyectos o en algunos, y para todas las IA o algunas.
- **Instrucciones comunes**, para todas las IA y por proyecto: llegan en cada encargo del
  chat, de las mejoras, de los delegados, de la revisión y de la ronda.
- No se toca la configuración propia de cada agente. Quitar una skill manda su carpeta a
  la Papelera.
- En el chat, el paso dice qué skill usó Claude Code.
- Terminal: `atalaya skills list|add|show|enable|disable|remove|instructions`.

## 0.31.0 — 2026-11-24

- **Adjuntos en el chat.** Pega una captura con ⌘V, arrastra archivos a la ventana o
  elígelos con 📎. Esperan sobre la caja, cada uno con su miniatura o su ficha, y puedes
  quitarlos antes de enviar. En la conversación quedan con el mensaje.
- **Cada IA los recibe a su manera:**
  - Claude Code lee la imagen, el PDF o el archivo desde su ruta, sin pedir permiso,
    también en solo lectura;
  - los agentes de terminal los leen desde su encierro, que ahora abre la carpeta de
    adjuntos solo para leer (el resto de los datos de Atalaya sigue cerrado);
  - los modelos por API reciben las imágenes dentro del mensaje (Anthropic, OpenAI,
    Gemini y compatibles) y el texto de los archivos de texto. Un PDF u otro archivo les
    llega solo por su nombre, con el aviso de elegir Claude Code para leerlo.
- Los adjuntos se guardan en `adjuntos/` dentro de la carpeta de datos de Atalaya, hasta
  25 MB cada uno. Atalaya no los borra por su cuenta.

## 0.30.1 — 2026-11-24

- Las herramientas del MCP que responden con un texto (como `resolve_decision` o los
  cambios de administración) lo entregan tal cual, sin comillas ni `\n` escapados. En el
  chat se lee como una frase.

## 0.30.0 — 2026-11-24

- **Mejoras y decisiones desde el chat.** La barra «Tablero», en la vista Agente,
  cuenta lo que espera algo de ti: decisiones pendientes, mejoras que te preguntan
  algo, en revisión, propuestas, aceptadas y en curso. Cada grupo abre su lista con los
  mismos botones de las vistas Mejoras y Decisiones. Filtra por el proyecto elegido.
- **Tarjetas bajo cada paso.** Cuando Claude Code crea, mueve o resuelve una mejora o
  una decisión, su tarjeta aparece bajo ese paso, con el estado al día y sus botones.
- **«Hazla aquí».** Le pide a Claude Code que trabaje la mejora en la conversación, sin
  pasar por el agente de Mejoras. «Preguntar» y «Pedir cambios» te dejan el mensaje
  empezado en la caja.
- **Claude Code puede resolver decisiones** (`resolve_decision`), solo si se lo pides y
  con tu visto bueno: la tarjeta dice «Aprobar la decisión #N» o «Descartar…».

## 0.29.0 — 2026-11-24

- **Claude Code reparte varios encargos a la vez.** Con «Claude Code coordina»
  encendido, cuando tiene varias cosas independientes las manda en una sola llamada y
  cada IA trabaja en paralelo:
  - consultas a varias IA de tus roles (hasta 6);
  - cambios a varios agentes, uno por proyecto, para que no se pisen en la misma carpeta.
- **El trabajo se reparte de verdad.** Los encargos que no nombran a nadie van cada uno a
  otro del rol, en vez de caer todos en el primero. Si uno falla, prueba con los demás, y
  un encargo fallido no detiene al resto.
- **En el chat se ven lado a lado.** Cada encargo del reparto tiene su recuadro en vivo,
  y la franja «Equipo» muestra a cada IA trabajando.
- Un reparto de cambios espera tu visto bueno, salvo en «Puede todo sin preguntar»: la
  tarjeta lista cada proyecto con su agente y su encargo.

## 0.28.0 — 2026-11-24

- **En el chat se ve quién hace qué.** Cada paso lleva el nombre de quien lo dio: Claude
  Code, uno de sus subagentes o la IA a la que le encargó algo.
  - Los pasos de un subagente salen sangrados bajo la llamada que lo lanzó.
  - Un paso en marcha termina en «…», y si falla queda en rojo.
- **La franja «Equipo».** Arriba de la conversación muestra quién trabaja ahora y quién
  ya terminó. Abajo, mientras espera, dice a quién está esperando Claude Code.
- **Las IA delegadas trabajan a la vista.** Lo que va escribiendo la IA a la que Claude
  Code le encargó una consulta o un cambio aparece en vivo en un recuadro bajo el
  encargo, con «Ver todo» para la salida completa.
- **Las aprobaciones dicen quién las pide.** Si el permiso lo pide un subagente, la
  tarjeta lo nombra.

## 0.27.0 — 2026-11-24

- **Los agentes ya pueden ejecutar órdenes dentro del aislamiento.** El aislamiento de
  macOS no les dejaba abrir una terminal virtual, así que Antigravity (y cualquier agente
  que use una) fallaba en cada orden con «failed to create PTY» y terminaba preguntando.
  Ahora puede: sigue sin poder escribir fuera de su copia ni leer tus llaves.
- **Las mejoras ya no se detienen en el primer constructor.** Si uno termina sin cambios,
  porque se trabó o preguntó algo, la mejora pasa enseguida al siguiente de tu lista de
  Constructor, con Claude Code al final si está.
- **Si ninguno la logra, te avisa y pasa al Agente.** La notificación dice que la mejora
  necesita tu respuesta. Al hacer clic se abre el Agente con la mejora, su pregunta y el
  proyecto elegido: lo que escribas lo recibe Claude Code con todo el contexto, retoma la
  mejora y la lleva hasta el final, moviéndola por el tablero.
  - En Mejoras, «Responder en el Agente» hace lo mismo desde la tarjeta.

## 0.26.0 — 2026-11-24

- **Las notificaciones salen a nombre de Atalaya.** Antes salían con el ícono del Editor
  de Scripts, y al hacer clic se abría ese programa. Ahora tienen el ícono de Atalaya y el
  clic abre Atalaya en lo que avisaban:
  - una ronda con decisiones nuevas, en Decisiones;
  - una mejora lista o fusionada, en Mejoras;
  - una falla, en Alertas;
  - el radar y el presupuesto, en sus vistas.
- **Puedes responderle al agente de Mejoras.** Si el agente necesita una decisión tuya,
  termina sin cambios y deja su pregunta en la tarjeta («Te pregunta»). Ahí mismo hay un
  cuadro para responderle: con «Responder y seguir», la mejora vuelve a la cola y el agente
  empieza de nuevo con tu respuesta. Las respuestas se acumulan, así que no te vuelve a
  preguntar lo mismo.
  - En una mejora en revisión, «Pedir cambios» le dice qué corregir en el próximo intento.
  - Tras un intento sin cambios, «Dar indicaciones» le agrega lo que quieras.
  - El agente ahora sabe que nadie le responde mientras trabaja: si le falta un detalle
    menor, elige lo razonable y lo explica, y solo pregunta si de verdad no puede avanzar.

## 0.25.0 — 2026-11-24

- **Claude Code coordina** (IA → «Claude Code coordina», apagado por defecto). Con la
  opción encendida, Claude Code en el chat del Agente le pide análisis y revisiones a las
  demás IA de tus roles y les encarga cambios acotados en la carpeta de un proyecto.
  Después revisa lo que devuelven antes de responderte. Así gasta menos de tu plan de
  Claude y prefiere a las gratis.
  - Solo le pide a quienes están en tus roles.
  - Encargar un cambio espera tu visto bueno en «Puede todo, pregunta antes de cambiar»,
    y en solo lectura solo puede consultar.
  - Los agentes trabajan aislados: solo escriben en la carpeta del proyecto.
  - Cada encargo se ve en el chat («Le pregunta a otra IA», «Le encarga un cambio a otra
    IA») y en Consumo como «Delegado».

  Si la dejas apagada, nada cambia: cada rol trabaja con quien elegiste.
- **Las mejoras avanzan por el tablero cuando el Agente las trabaja.** Antes, el chat las
  anotaba como propuestas, las hacía y las dejaba ahí. Ahora las mueve:
  - «En curso» al empezar;
  - «En revisión» cuando el cambio espera tu visto bueno, con la rama o el PR;
  - «Hechas» al terminar;
  - cada vez deja una nota de qué hizo o qué falta.

  Si la conversación termina o se detiene a medias, la mejora vuelve a «Aceptada» con esa
  nota, en vez de quedar como si alguien la siguiera trabajando.
- En Mejoras, lo que está en revisión sin pull request (un cambio hecho desde el chat) se
  puede marcar como hecho.

## 0.24.0 — 2026-11-24

- **Consumo** muestra quién hizo el trabajo del mes: cada IA con su porcentaje de las
  llamadas y de los tokens, en qué tareas trabajó (chat, ronda, mejoras, revisión) y si es
  gratis, va con tu cuenta o se cobra por uso. Arriba, qué parte del trabajo fue gratis.
  Así se ve si de verdad se reparten el trabajo o lo hace todo una sola.
- Claude Code ahora queda anotado en el consumo cada vez que trabaja: en el chat, la
  ronda, las mejoras, las revisiones y los resúmenes del radar, con su modelo y sus
  tokens. Antes no se anotaba nunca, así que el consumo solo veía a las demás IA. Como lo
  cubre tu suscripción, no suma al gasto.
- Los agentes que usan tu cuenta ya no aparecen con «precio sin definir»: dicen «tu
  cuenta» o «gratis».
- En el chat del Agente, cada respuesta dice qué IA la dio («Atalaya · Claude Code»).
- `atalaya ai usage` también muestra el reparto.

## 0.23.0 — 2026-11-24

- **Agente** sigue trabajando aunque cambies de vista. Antes, al salir, la conversación
  se borraba y el trabajo quedaba sin nadie que lo mostrara. Ahora la conversación, lo
  que escribías y el trabajo en curso siguen ahí al volver. En la barra, junto a
  «Agente», un punto avisa que está trabajando y un número que algo espera tu visto
  bueno.
- Puedes escribirle mientras trabaja: el mensaje queda en cola y le llega cuando
  termina. Para cambiar de rumbo ya, envíalo y pulsa **Detener**: se corta lo que hacía
  y sale tu mensaje. Un mensaje en cola se puede quitar antes de que salga.
- Al volver a Agente se releen los roles, por si cambiaste el asistente en la vista IA.

## 0.22.2 — 2026-11-24

- Los modelos `:cloud` de Ollama ya no cuentan como gratis: corren en la nube de Ollama y
  se cobran por uso (hoy no tiene capa gratis, solo créditos con Pro o Max). En el
  selector de Qwen Code aparecen como «nube de Ollama, de pago», y la etiqueta «Gratis»
  de un agente depende del modelo que eligió.
- Un error de saldo (402, «Add usage credits», sin suscripción) se explica en simple:
  ese modelo se cobra y la cuenta no tiene saldo ni plan.

## 0.22.1 — 2026-11-24

- **Cursor** aparece como gratis: su plan Hobby no se paga y trae pedidos de agente
  limitados.
- Probar una conexión de modelos avisa si el modelo elegido no está en ese servicio y
  dice cuáles tiene. Antes se aceptaba cualquier texto, incluso una llave pegada donde va
  el modelo.
- El selector de modelo de Qwen Code solo ofrece modelos de este Mac que su servidor
  tenga de verdad.
- `localhost` pasa como `127.0.0.1` al modelo de un agente: Node prueba primero `::1`, y
  Ollama y LM Studio no escuchan ahí. La dirección predeterminada de los modelos locales
  es ahora `127.0.0.1`.

## 0.22.0 — 2026-11-24

- **Lo gratis primero**. En IA, los agentes que se pueden usar gratis van arriba, con la
  etiqueta «Gratis» y cómo se logra: Antigravity (cuenta personal de Google), Codex
  (ChatGPT Free), Copilot (Copilot Free), OpenCode (modelos gratis de OpenCode Zen),
  Gemini CLI (llave gratis de AI Studio) y Qwen Code (modelo de tu Mac). En Conexiones,
  los servicios con uso gratis van primero: OpenRouter (modelos «:free»), Groq, Mistral,
  GitHub Models y Z.ai. En «Agregar…» de cada rol, lo gratis aparece primero y marcado.
- Estrategia nueva **«Gratis primero»**, la predeterminada: de la lista de cada rol,
  Atalaya intenta antes lo gratis y respeta el orden dentro de cada grupo. «El más
  barato» también cuenta lo gratis como costo cero.
- **Qwen Code con un modelo propio**, de tu Mac (Ollama, LM Studio) o de la nube (una
  conexión de modelos guardada, o los modelos «:cloud» de Ollama). Se elige en IA, en la
  fila de Qwen Code, o con `atalaya ai model qwen-code`. No necesita iniciar sesión.
  Atalaya lo lanza con `--bare`, sin las skills ni el contexto de la persona: con un
  modelo local cada pedido baja de unos 28 000 tokens a 7000, y la respuesta, de minutos
  a segundos. El plan gratis de Qwen OAuth se cerró el 15 de abril de 2026; la app ya no
  lo ofrece.
- Los campos internos de una conexión (como la cuenta del Llavero del modelo de Qwen) no
  aparecen en el formulario, pero se conservan al editarla.

## 0.21.0 — 2026-11-23

- **Comprobar valida de verdad**: en IA, el botón le hace una pregunta corta al agente
  (en solo lectura, dentro del encierro) y muestra «Funciona: respondió …» o, si falla,
  el motivo en palabras simples (falta de sesión, de plan, de cuota o de llave). Antes
  solo volvía a mirar si estaba instalado.
- `atalaya ai verify [agente]` hace lo mismo desde la terminal, con todos los agentes
  instalados a la vez.
- **Amp, Factory Droid y OpenCode** ahora saben si tienen la sesión iniciada:
  Atalaya les pregunta (`amp account list`, `droid doctor --checks auth`,
  `opencode auth list`) en vez de decir «inicia sesión si no lo has hecho».
- Copilot se lanza con `-s`, para que su aviso de modelo no se mezcle con la respuesta.
- **Modelos locales**: LM Studio y Jan se reconocen aunque su servidor esté apagado.
  La app dice si la app está abierta o cerrada, qué modelos tienes descargados (y cuáles
  siguen bajando) y cómo encender su servidor. LM Studio tiene un botón «Encender
  servidor»; Jan se enciende desde su propia app. El panel «En este Mac» tiene un botón
  Actualizar.

## 0.20.1 — 2026-11-23

- **Gemini CLI con llave**: en modo automático Gemini CLI solo lee la llave de la
  variable `GEMINI_API_KEY`, no la que se guarda en su propia ventana. Su conexión
  acepta ahora una llave opcional, guardada en el Llavero, y Atalaya se la entrega solo a
  Gemini CLI. Con Vertex AI o una cuenta de empresa no hace falta.
- Gemini CLI se lanza con `--skip-trust`: sin eso, en una carpeta nueva bajaba a modo
  «preguntar» y no hacía nada. Para responder usa `--approval-mode plan`, que es de solo
  lectura.

## 0.20.0 — 2026-11-23

- **Antigravity CLI** (`agy`), el agente de Google para tu cuenta personal (gratis,
  Google AI Pro o Ultra). Atalaya sabe si tiene la sesión iniciada preguntándole sus
  modelos. Desde el 18 de junio de 2026 Google ya no acepta cuentas personales en
  **Gemini CLI**: queda para llaves de Gemini, Vertex AI y cuentas de empresa, y la app
  lo explica al iniciar sesión.
- **Kimi Code** reemplaza al kimi-cli, que quedó sin mantenimiento. Si Atalaya encuentra
  el antiguo, avisa que hay que instalar el nuevo y no lo usa. Su salida se limpia de
  las marcas «•».
- **Copilot** inicia sesión con `copilot login`. Antes usaba la llave de `gh`, que suele
  no tener el permiso «Copilot Requests», y respondía con error 401.
- **Arreglo importante del encierro**: Codex, Antigravity y Cursor no funcionaban
  dentro de él. El encierro bloqueaba el Llavero, y macOS lo usa también para validar
  los certificados HTTPS. Ahora cada agente puede pedir su propia sesión al Llavero;
  macOS solo le entrega lo que guardó él. Probado con las cuentas reales de Codex,
  Antigravity y Cursor. En solo lectura siguen sin poder escribir en los proyectos.
- Probar una conexión funciona con todos los agentes (antes fallaba con Qwen, Kimi,
  Copilot, Cursor, Droid y Amp) y avisa si falta iniciar sesión.
- En **Agente**, el modo «Puede todo en tus proyectos» usa ahora el comando de trabajar
  del agente. Antes usaba el de responder, que en algunos agentes no deja cambiar nada.
- Cursor responde con `--trust --mode ask`, porque sin `--trust` se detenía a preguntar
  si confiaba en la carpeta.
- **Vista IA en pantallas angostas**: los botones bajan a otra línea cuando no caben, en
  vez de quedar cortados.

## 0.19.0 — 2026-11-23

- **Conectar IA con tu cuenta, sin llaves de API.** La vista **IA** muestra los
  agentes de terminal que funcionan con la suscripción que ya pagas:
  - Claude Code, Codex (ChatGPT), Gemini CLI (Google), Qwen Code, Kimi, GitHub
    Copilot, Cursor, OpenCode, Factory Droid y Amp;
  - para cada uno, si está instalado y si tiene la sesión iniciada;
  - **Instalar** e **Iniciar sesión** abren la Terminal con el comando del agente, y
    se entra en el navegador como en cualquier app. Atalaya nunca lee sus
    credenciales: le pregunta al programa o mira si existe su archivo de sesión;
  - **Usar en Atalaya** lo deja listo para los roles.
- **Modelos de este Mac**: Atalaya encuentra Ollama, LM Studio, Jan y llama.cpp, lista
  sus modelos y los agrega con un clic. Si la app está instalada pero cerrada, ofrece
  abrirla.
- Los agentes de terminal sirven ahora en **todos los roles**, no solo en el de
  constructor: revisor, validador y asistente. Para responder corren encerrados y en
  solo lectura.
- **Agente** conversa con cualquier agente de terminal, con el texto en vivo y la
  conversación recordada. Tiene dos modos: **Puede todo en tus proyectos** (escribe solo
  en las carpetas de los proyectos) y **Solo lectura**. Claude Code y los demás
  recuerdan su modo por separado.
- **Por API**: el formulario de «Compatible con OpenAI» trae 18 servicios listos
  (OpenRouter, DeepSeek, Groq, Mistral, xAI, Moonshot/Kimi, Qwen, Z.ai/GLM, MiniMax,
  Together, Fireworks, Cerebras, Perplexity, NVIDIA, Hugging Face, GitHub Models,
  Vercel AI Gateway y Cohere).
- Encierro de los agentes: las carpetas de los proyectos quedan protegidas contra
  escritura cuando el agente solo responde, aunque estén dentro de una carpeta
  escribible.
- Codex corre sin su propio encierro, dentro del de Atalaya, porque macOS no permite
  anidarlos.
- Terminal: `atalaya ai clis`, `atalaya ai login <agente>` y `atalaya ai local`.
  `atalaya ai ask` también acepta agentes.

## 0.18.0 — 2026-11-23

- **Agente** es ahora una terminal de IA: Claude Code trabaja con todas sus
  herramientas desde la app. Lee y edita código, ejecuta órdenes, usa git, corre pruebas
  y builds, y usa todas las herramientas de Atalaya. Tiene acceso a las carpetas de
  todos los proyectos registrados.
- Tres modos:
  - **Puede todo, pregunta antes de cambiar** (el normal): cada acción que cambia algo
    aparece en la conversación con su detalle (la orden, el cambio en el archivo) y
    espera **Permitir**, **Permitir siempre en esta conversación** o **Rechazar**, con
    un motivo opcional que el agente lee;
  - **Puede todo sin preguntar**: igual, sin esperar tu visto bueno, salvo lo
    irreversible o lo que toca producción;
  - **Solo lectura**: como antes.
- El texto llega mientras se escribe. Cada herramienta muestra qué hace y su salida,
  plegada cuando es larga.
- **Detener** corta al agente y todo lo que lanzó. La conversación puede seguir
  después.
- Herramienta MCP nueva, `aprobar_accion`, y tabla `approvals`, que comunican al agente
  con la app.
- Los modelos por API siguen disponibles en Agente para conversar con el estado de los
  proyectos, sin cambiar nada.

## 0.17.0 — 2026-11-23

- `INSTALAR.md`: cómo instalar el paquete sin firma de Apple, el primer uso, qué hace
  Atalaya sola, dónde quedan los datos, cómo desinstalar y qué mirar para dar opinión.
- `README.md`: arquitectura, cómo compilar, cómo probar sin tocar una instalación real
  y cómo agregar un proveedor.
- `atalaya doctor` revisa cada rol de IA (quién lo hace o por qué nadie puede) en vez
  de exigir Claude Code.
- Textos de ayuda de la terminal al día con los roles.

## 0.16.0 — 2026-11-23

- Orquestación: una vista nueva, **IA**, para elegir quién hace cada trabajo. Cada rol
  tiene una lista en orden y una estrategia (el primero disponible, el más barato o
  rotar). Si el elegido no está instalado, falla o pasó su presupuesto, Atalaya sigue
  con el siguiente.
  - **Constructor**: hace las mejoras. Claude Code o un agente de terminal; los que no
    son Claude Code trabajan aislados y solo pueden confirmar commits en su rama (no
    tocan los hooks ni la configuración del repo).
  - **Revisor**: otro modelo lee el cambio antes de que llegue a ti y da su veredicto,
    que va a la descripción del pull request y a Mejoras. Si pide cambios, el
    constructor tiene un intento más. Nunca es el mismo modelo que construyó.
  - **Validador**: la ronda diaria puede hacerla cualquier modelo; los modelos por API
    reciben el estado de los proyectos en JSON y dejan sus decisiones igual que Claude.
  - **Asistente**: responde en Agente. En cada conversación se puede elegir otro modelo.
- Contexto común: el mismo paquete (revisiones, mejoras, decisiones y eventos) para
  cualquier modelo.
- El chat y la ronda pasan su propia configuración MCP a Claude Code: nadie tiene que
  configurar el MCP a mano.
- MCP: `ai_usage` y `ai_roles`, de solo lectura. Terminal: `atalaya ai roles` y
  `atalaya ai role <rol> <candidatos…> --strategy …`.
- Lo de siempre no cambia si no se toca nada: Claude Code construye, valida y
  conversa, y nadie revisa hasta que se asigne un revisor.

## 0.15.0 — 2026-11-23

- Varias IA: Anthropic, OpenAI, Google Gemini, cualquier servicio compatible con
  OpenAI (OpenRouter, DeepSeek, Groq, Mistral…) y modelos locales con Ollama o LM
  Studio. Probar la conexión lista sus modelos.
- Consumo por IA: cada llamada que hace Atalaya queda anotada (conexión, modelo,
  tarea, proyecto, tokens y costo). La vista Consumo muestra el gasto del mes por
  modelo y por tarea, un presupuesto por conexión y los precios conocidos.
  - Los precios no se inventan: los fija la persona, los informa OpenRouter o, en
    modelos locales, son 0. Sin precio se muestran los tokens y la marca «precio sin
    definir»; al fijarlo, las llamadas del mes se recalculan.
  - Claude Platform sigue en la misma vista para cuadrar los números.
- Agentes de terminal además de Claude Code: Codex CLI, Gemini CLI, OpenCode o uno
  propio con plantilla de comando. Corren dentro de un aislamiento de macOS
  (`sandbox-exec`): solo escriben en su copia de trabajo y en sus cachés, y no leen
  `~/.ssh`, `~/.aws`, las credenciales de git, el Llavero ni los datos de Atalaya.
- Terminal: `atalaya ai models|ask|usage|price|budget`. `connections add` acepta la
  llave por la entrada estándar cuando no hay terminal, para usarlo en scripts.

## 0.14.0 — 2026-11-23

- Conexiones: una vista nueva para agregar, probar, editar y quitar las cuentas y
  servicios que usa Atalaya, sin terminal. Puede haber varias del mismo proveedor.
  - Repos: GitHub, GitLab (el público o uno propio) y Gitea/Forgejo.
  - Hosting: Vercel, Netlify y servidores propios (URL de salud y, con SSH, carga y
    disco; usa el agente SSH del sistema, Atalaya no lee `~/.ssh`).
  - Bases: Supabase, Firebase, Postgres, MySQL/MariaDB y archivos SQLite locales.
  - Avisos: correo por cualquier servidor SMTP (Gmail queda como preajuste) y Telegram.
  - IA y agentes: se registran y prueban aquí; la orquestación llega en 0.15 y 0.16.
  - Las llaves que ya estaban en el Llavero aparecen solas como conexiones, sin
    moverlas. Quitar una conexión no borra su llave salvo que se pida.
- Cada proyecto elige sus conexiones (repo, hosting, base, respaldo) en su ficha.
  Vigía revisa además bases propias, servidores y repos de GitLab/Gitea, y los
  respaldos cubren Postgres (`pg_dump`), MySQL (`mysqldump`) y SQLite.
- Carpetas de proyectos: eliges dónde guardas tus proyectos, Atalaya los busca (una
  carpeta con `.git` es un proyecto), los registra y propone sus vínculos. Sin
  proyectos, la app abre en una bienvenida. Los repos nuevos se clonan en la primera
  carpeta elegida.
- Terminal: `atalaya connections list|providers|add|test|remove`, `atalaya link`,
  `atalaya unlink`, `atalaya roots add|list|remove` y `atalaya scan [--add]`.
- El equipo de Vercel se elige en la conexión; GitHub lista también los repos de tus
  organizaciones y colaboraciones; los ejemplos ya no nombran proyectos propios.
- `ATALAYA_KEYCHAIN_SERVICE` permite probar con un Llavero aparte.

## 0.13.1 — 2026-11-23

- Administración: la llave de la puerta se guarda en Vercel como variable sensible,
  que nadie puede volver a leer en el panel ni por la API. Al conectar una app que ya
  tenía la llave como variable legible, Atalaya la reemplaza con el mismo valor.

## 0.13.0 — 2026-11-23

- Administración: cada proyecto tiene un panel para manejar los controles de su app
  (planes, estados, módulos encendidos o apagados, anuncios…) sin escribir en su base.
  La app los ofrece por una puerta propia, `/api/atalaya`, protegida con una llave que
  Atalaya genera y guarda en el Llavero y en Vercel. Los cambios delicados piden
  confirmación. El protocolo está en
  `docs/puerta-admin.md`.
- Terminal: `atalaya admin connect | show | set | key`.
- MCP: `admin_controls` y `admin_set`, para que las sesiones de Claude vean y cambien
  esos controles cuando se les pide.

## 0.12.0 — 2026-11-23

- Alertas: el botón principal ahora es "Resolver", que crea la mejora y pone al agente
  a arreglarla. "Silenciar" queda para lo que solo se quiere dejar de contar.
- Autonomía: opción en Ajustes para que las mejoras se fusionen solas y salgan a
  producción cuando todas las comprobaciones de su pull request pasan.

## 0.11.2 — 2026-11-23

- Decisiones: aprobar una decisión que antes era solo informativa ahora la convierte
  en mejora y pone al agente a resolverla ("Aprobar y resolver"). Sigue existiendo
  "Dar por vista" para las que no necesitan trabajo.
- La ronda del agente prefiere proponer mejoras que se puedan trabajar.

## 0.11.1 — 2026-11-23

- Firma propia: `scripts/crear-firma.sh` crea una firma local y el instalador la usa
  si existe, para que macOS conserve los permisos de una versión a la siguiente.

## 0.11.0 — 2026-11-23

- Decisiones: el agente hace una ronda diaria, revisa todos los proyectos y deja
  propuestas que esperan tu visto bueno. Al aprobar, Atalaya crea la mejora, publica
  el comentario en GitHub o reconoce la alerta. La propuesta se puede editar antes.
- Radar: vigila páginas de la competencia, detecta cuando cambian y resume qué cambió.
- Comprobación del sistema (`atalaya doctor` y en Ajustes): permisos de carpetas,
  herramientas, llaves y servicio. Al abrir la app pide de una vez los permisos que falten.
- Corrección: el aislamiento del agente le bloqueaba su propia carpeta de trabajo, que
  ahora vive fuera de la carpeta de datos.
- MCP: `list_decisions`, `propose_decision`, `github_issues` y `radar_events`.

## 0.10.1 — 2026-11-23

- Corrección: con un mes sin uso, el gasto de Claude Platform se mostraba como "-0.00".

## 0.10.0 — 2026-11-23

- Respaldos automáticos: copia diaria de las bases de Supabase y Firestore en la
  carpeta de datos de Atalaya, con los 7 más recientes por proyecto y alerta si falla.
- Revisión de certificados SSL, con aviso antes de que venzan.
- Revisión de migraciones: avisa si el repo tiene migraciones sin aplicar en producción
  (Drizzle y Supabase).
- La revisión de Supabase incluye el tamaño de la base y avisa cerca del límite gratuito.
- Tendencia del sitio en cada proyecto: disponibilidad y tiempo de respuesta por día.
- Clonar desde la ventana un repo vigilado solo en remoto.
- Avisos al teléfono por Telegram para las fallas graves.
- Aislamiento del agente a nivel del sistema: solo escribe en su copia de trabajo, no
  puede leer llaves y solo sale a internet hacia registros de paquetes y GitHub.
- El error de Claude Platform explica cuando la llave no es de administrador.

## 0.9.0 — 2026-11-23

- Claude Platform: pantalla Consumo con el gasto del mes de la API de Claude,
  solicitudes y tokens por modelo y gasto diario de los últimos 14 días.
- Presupuesto mensual con aviso por notificación y correo al 80 % y al 100 %.
- El vigía refresca el consumo cada hora y el resumen diario lo incluye.
- MCP: herramienta `claude_usage`. Comandos `atalaya claude status` y `budget`.

## 0.8.3 — 2026-11-23

- Barra lateral colapsable: se contrae a íconos con el botón "Contraer" o con ⌘B,
  y recuerda la preferencia.
- Una sola Atalaya a la vez: abrirla de nuevo muestra la que ya está corriendo.
  Antes, una segunda copia marcaba como interrumpido el trabajo de la primera.
- Al salir de Atalaya se detienen los agentes en marcha, en vez de quedar huérfanos.

## 0.8.2 — 2026-11-23

- Interfaz adaptable: el tablero y los paneles se reparten según el ancho de la
  ventana, los textos largos ya no se cortan ni se salen de su caja y las tablas
  anchas se desplazan dentro de su panel.
- En ventanas medianas la barra lateral se reduce a íconos; la ventana admite
  tamaños más pequeños.
- Tras fusionar una mejora, Atalaya pone al día la carpeta local del proyecto,
  solo si está en la rama principal y sin cambios pendientes.

## 0.8.1 — 2026-11-23

- El agente tiene autonomía completa dentro de su copia de trabajo: puede ejecutar
  cualquier orden (instalar, actualizar, probar, compilar) y buscar documentación.
- Corrección: al abrir Atalaya desde el Finder el agente no encontraba Node ni npm;
  ahora usa el mismo PATH que una terminal.
- Subir y desplegar siguen bloqueados para el agente, ahora también a nivel técnico.
- Notificación y correo cuando una mejora queda lista para revisión.
- El informe del agente explica el problema, la solución, cómo la verificó y qué revisar.

## 0.8.0 — 2026-11-23

- Mejoras con agente: al aceptar una mejora, Claude Code la trabaja solo en una
  copia aparte del repo y en una rama nueva, y Atalaya sube la rama y abre el
  pull request. El cambio llega a producción solo cuando se fusiona.
- Tablero con el flujo completo: propuestas, en curso, en revisión y hechas, con
  el avance del agente en vivo y su informe en cada tarjeta.
- Botón para fusionar el pull request desde Atalaya, con confirmación.
- El vigía pasa a "hecha" las mejoras cuyo pull request ya se fusionó en GitHub.
- Comando `atalaya work <número>` para trabajar una mejora desde la terminal.

## 0.7.0 — 2026-11-23

- Ícono en la barra de menú con el número de alertas sin reconocer y un menú
  para abrir Atalaya, revisar todo ahora y salir.
- Cerrar la ventana la oculta en vez de apagar la app; se sale desde el menú.

## 0.6.0 — 2026-11-23

- Firebase: estado del proyecto y de las reglas de Firestore y Storage. Alerta si
  permiten escribir a cualquiera y avisa si difieren de las del repo.
- Control de versiones: versión única, este historial y etiquetas por versión.
- La versión se muestra en la barra lateral de la app.

## 0.5.0 — 2026-11-23

- Instalador `.pkg`: app en Aplicaciones, comando en `/usr/local/bin` y vigía como servicio.
- Comando `atalaya service` para instalar, quitar y consultar el vigía.
- GitHub: listar y crear repos, vigilar repos sin carpeta local y revisión del último commit.
- Agente: chat con Claude Code dentro de la app, con herramientas de lectura.
- Proyectos: panel de repositorios, proyectos solo remotos y botón Quitar.

## 0.4.0 — 2026-11-23

- Consola nueva con la marca Atalaya: paleta nocturna, tipografías e ícono propios.
- Pantallas Vigía, Alertas, Proyectos, Mejoras y Ajustes.

## 0.3.0 — 2026-11-23

- Llaves en el Llavero de macOS.
- Revisiones de despliegues de Vercel y de bases de Supabase.
- Correo por Gmail: alertas agrupadas y resumen diario.
- Interruptores de funciones en Global Config de Vercel.
- Alertas reconocibles y tablero de mejoras.

## 0.2.0 — 2026-11-22

- Vigilancia: revisiones de sitio, git y dependencias, con historial y notificaciones.
- Comandos `check`, `watch` y `report`.

## 0.1.0 — 2026-11-22

- Núcleo en Rust con registro de proyectos en SQLite.
- Línea de comandos, servidor MCP de solo lectura y app de escritorio.
