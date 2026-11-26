# Instalar y probar Atalaya

Atalaya vigila tus proyectos de software desde tu Mac. Revisa sitios, despliegues, bases
de datos, repos y dependencias, y te avisa cuando algo cambia o se rompe. Con IA puede
también proponer mejoras, trabajarlas en una rama aparte y dejarte un pull request para
que decidas.

Esta es una versión de prueba. Lo que más sirve es tu opinión: qué entendiste, qué no
y qué te faltó (al final hay una lista).

## Qué necesitas

- Un Mac con chip Apple (M1 o más nuevo) y macOS reciente. El paquete no funciona en
  Macs con Intel.
- **git**. Si no lo tienes, macOS lo ofrece al escribir `git` en la Terminal, o con:

  ```bash
  xcode-select --install
  ```

- Opcional, según lo que quieras probar:
  - **Una IA**. Lo más simple es usar la suscripción que ya tienes: Claude Code
    (Claude), Codex (ChatGPT), Antigravity CLI (Google), Copilot, Cursor, Kimi Code o
    Qwen. Con una cuenta personal de Google se usa Antigravity CLI: Gemini CLI ya solo
    acepta llaves de Gemini y cuentas de empresa.
    Atalaya los instala y los conecta desde la vista **IA**. También sirven los modelos
    gratis de tu Mac (Ollama, LM Studio, Jan) o cualquier API con llave.
  - **GitHub CLI** (`gh`), si prefieres no pegar una llave de GitHub.
  - `pg_dump` o `mysqldump`, para respaldar bases Postgres o MySQL.

## Instalar

1. Abre `Atalaya-<versión>.pkg`.
2. macOS dirá que no puede abrirlo porque el desarrollador no está identificado. Es
   normal: el paquete no está firmado por Apple. Ve a **Configuración del Sistema →
   Privacidad y seguridad**, baja hasta el aviso sobre Atalaya y pulsa **Abrir
   igualmente**. Confirma con tu contraseña.
3. Sigue el instalador. Deja:
   - la app en **Aplicaciones**;
   - el comando `atalaya` en la Terminal;
   - el **vigía**, un servicio en segundo plano que revisa tus proyectos aunque la app
     esté cerrada.
4. Abre **Atalaya** desde Aplicaciones. Si macOS vuelve a avisar, repite el paso 2.

Durante el uso macOS puede pedirte permisos. Conviene aceptarlos:

- **Llavero**: Atalaya guarda ahí las llaves de tus servicios. Elige «Permitir siempre».
- **Carpetas** (Documentos, Descargas, Escritorio): para leer tus proyectos.
- **Notificaciones**: para los avisos. Salen a nombre de Atalaya, y al hacer clic se abre
  Atalaya en lo que avisaban (Decisiones, Mejoras, Alertas…). Si no las ves, revisa que
  Atalaya tenga las notificaciones permitidas en Configuración del Sistema → Notificaciones.

## Primer uso

La app abre en una bienvenida de cuatro pasos:

1. **Carpetas.** Elige dónde guardas tus proyectos. Atalaya busca ahí, hasta tres
   niveles, las carpetas que tienen un repositorio git.
2. **Proyectos.** Marca los que quieres vigilar y pulsa **Registrar**. Si detecta que
   usan Vercel, Netlify, Supabase o Firebase, propone vincularlos.
3. **Conexiones** (puedes saltarlo). Aquí van tus cuentas y servicios:
   - repos (GitHub, GitLab, Gitea);
   - hosting (Vercel, Netlify o tu propio servidor);
   - bases (Supabase, Firebase, Postgres, MySQL, SQLite);
   - avisos (correo, Telegram);
   - IA y agentes.

   Cada una se prueba al guardarla. Las llaves van al Llavero y no vuelven a mostrarse.
4. **IA** (puedes saltarlo). En la vista **IA** conectas tus IA:
   - **Con tu cuenta**: pulsa **Instalar** si falta y luego **Iniciar sesión**. Se abre
     la Terminal con el inicio de sesión del agente y entras en el navegador, como en
     cualquier app. Al final, **Usar en Atalaya**. **Comprobar** le hace una pregunta
     corta al agente y te dice si responde o qué le falta (sesión, plan, cuota o llave);
   - **En este Mac**: los modelos de Ollama, LM Studio o Jan aparecen solos cuando su
     servidor local está encendido. Si está apagado, Atalaya te dice cómo encenderlo
     (en LM Studio con el botón **Encender servidor**; en Jan, desde Settings → Local API
     Server) y qué modelos tienes descargados;
   - **Por API**: en Conexiones, eligiendo el servicio. Con uso gratis: Groq,
     OpenRouter (modelos «:free»), Gemini Flash, Mistral, SambaNova, Cloudflare Workers AI,
     NVIDIA y Z.ai. Cada uno tiene su enlace para crear la llave, y la llave la pegas tú.
     Varias a la vez suman capacidad: cuando una se queda sin cuota, Atalaya pasa a la
     siguiente.

   Lo gratis aparece primero y marcado «Gratis». Para un constructor sin costo:
   **Qwen Code** con un modelo de tu Mac. Instálalo, y en su fila elige el modelo en
   **Modelo** (no hace falta iniciar sesión). En un Mac de 16 GB conviene un modelo
   chico, como `qwen3.5:4b` de Ollama, y subir en Ollama Settings → Context length
   a 32k.

   Después eliges quién hace cada trabajo:
   - **constructor**: trabaja las mejoras;
   - **revisor**: otro modelo revisa el cambio antes de que llegue a ti;
   - **validador**: hace la ronda diaria;
   - **asistente**: responde en el chat.

Después, **Vigía** muestra el estado de todo y **Alertas** lo que falla. En cada
proyecto, en **Proyectos**, eliges qué conexiones usa: su repo, dónde se publica, su base
y dónde se respalda.

## Qué hace solo y qué no

- El vigía revisa cada cierto tiempo y avisa cuando algo cambia.
- Una vez al día, el validador revisa todo y deja propuestas en **Decisiones**. Nada se
  hace sin que lo apruebes.
- Si configuraste Telegram, cada decisión nueva te llega al teléfono con botones para
  aprobarla o descartarla. Solo obedece a tu chat; escríbele `/decisiones` al bot para ver
  las pendientes. Se apaga desde Decisiones o con `atalaya telegram decisiones no`.
- Cuando apruebas una mejora, el constructor trabaja en una copia aparte del repo, en
  una rama nueva:
  - si es otro agente, como Codex, trabaja aislado: solo escribe en su copia y no ve
    tus llaves (solo puede pedir al Llavero su propia sesión);
  - antes de subir, Atalaya revisa que el cambio no lleve llaves, tokens ni archivos
    como `.env`; si los lleva, la rama se queda en tu Mac y la mejora te dice dónde;
  - el resultado llega como pull request;
  - nada llega a producción hasta que tú lo fusiones.

  El constructor trabaja solo y pregunta lo mínimo. Si concluye que no hace falta cambiar
  el código (ya estaba resuelto, o la causa está fuera del repositorio), la mejora queda
  como hecha con su explicación, sin pasársela a otro. Si termina sin cambios porque se
  trabó o le faltó algo, la mejora pasa enseguida al siguiente de tu lista. Solo si
  ninguno la logra, Atalaya te avisa con una notificación: al hacer clic se abre el
  **Agente** con la mejora y lo que preguntó, y al responder, Claude Code la retoma y la
  lleva hasta el final. También puedes responder en la tarjeta («Responder y seguir»,
  que la vuelve a poner en la cola) o abrirla en el Agente con «Responder en el Agente».
  Si el resultado no te convence, «Pedir cambios» le dice qué corregir.
- **Tu carpeta al día**: cuando se fusiona un cambio, Atalaya lo trae a la carpeta del
  proyecto en tu Mac. Solo avanza si no pisa nada tuyo: lo que tienes sin confirmar y no
  choca con el cambio se queda como estaba. Si no puede (estás en otra rama, tienes commits
  propios o algo choca), lo reintenta en cada vuelta del Vigía y deja en **Decisiones** un
  «Poner al día» que explica qué lo impide. Ese trabajo lo hace Atalaya, nunca un agente.
- **IA sin cuota**: si a un agente o modelo se le acaba su cuota gratis o su límite de uso,
  Atalaya lo deja en pausa hasta que se renueve y le pasa el trabajo al siguiente de la
  lista. En **Consumo** ves hasta cuándo y el último error de cada IA; con «probar ya» le
  quitas la pausa antes.
- **Historial** (en el Agente): cada conversación queda guardada. Puedes buscar en todas,
  abrir una y seguir desde ahí, o exportarla a Markdown.
- **Agente** es una terminal de IA dentro de la app. Con Claude Code puedes pedirle lo
  mismo que en la terminal: revisar un error, arreglarlo, actualizar dependencias o
  correr pruebas. Con otros agentes (Codex, Antigravity, Cursor…) también trabaja, aunque sin
  pedirte permiso paso a paso: o puede todo en tus proyectos, o solo lee. Con Claude
  Code eliges qué puede hacer:
  - **Puede todo, pregunta antes de cambiar** (el normal): cada cambio, sea editar un
    archivo, ejecutar una orden o tocar un interruptor, aparece en la conversación y
    espera que lo permitas o lo rechaces;
  - **Puede todo sin preguntar**: no espera tu visto bueno. Úsalo solo si confías en
    la tarea;
  - **Solo lectura**: solo mira.

  Mientras trabaja puedes ir a otras vistas: sigue trabajando, y al volver está todo. Si
  le escribes mientras trabaja, el mensaje le llega cuando termina. **Detener** lo corta
  en cualquier momento, y si dejaste un mensaje esperando, sale enseguida.

  Cuando el Agente trabaja una mejora, la mueve por el tablero de **Mejoras**: «En
  curso» al empezar, «En revisión» cuando el cambio espera tu visto bueno y «Hechas» al
  terminar, cada vez con una nota de lo que hizo. Si la conversación termina a medias,
  la mejora vuelve a «Aceptada» con lo que falta.
- **Claude Code coordina** (en IA, viene apagado): si lo enciendes, Claude Code en el
  chat reparte el trabajo con las demás IA de tus roles. Les pide análisis y revisiones,
  y les encarga cambios acotados en la carpeta de un proyecto; después revisa lo que
  devuelven y te responde. Solo le pide a quienes pusiste en tus roles, y cada encargo
  de cambio espera tu visto bueno, salvo en «Puede todo sin preguntar». Si lo dejas
  apagado, cada rol trabaja con quien elegiste y nada más.

  Si hay varias cosas independientes, las reparte a la vez: varias consultas, o varios
  cambios en proyectos distintos (uno por proyecto). Cada IA trabaja en paralelo, y lo
  que no le encarga a nadie en particular se reparte entre los de tu rol.
- **Quién hace qué en el chat.** Cuando trabaja más de uno, arriba de la conversación
  aparece la franja **Equipo**: Claude Code, sus subagentes y las IA a las que les
  encargó algo, cada una «trabajando» o «listo». Cada paso lleva el nombre de quien lo
  dio. Los pasos de un subagente salen sangrados bajo la llamada que lo lanzó. Lo que va
  escribiendo una IA delegada se ve en vivo en un recuadro bajo el encargo. Si un
  subagente pide permiso, la tarjeta dice que lo pide él.
- **Atalaya para los demás agentes.** Codex, Gemini CLI (en las consultas) y los agentes
  propios pueden consultar Atalaya por su MCP mientras trabajan: los proyectos, su
  estado, las mejoras, las decisiones y el consumo. Solo para leer, y sin que el agente
  pueda abrir la carpeta de Atalaya. No hay que configurar nada en cada agente; a uno
  propio se le pasa con `{mcp_config}` en su plantilla.
- **Servidores MCP para todas las IA** (vista Skills y MCP). Agrega Supabase, GitHub,
  Context7, Playwright o el tuyo, pruébalo y actívalo: Claude Code y los agentes lo
  reciben cuando trabajan, sin configurar nada en cada uno. Su llave queda en el
  Llavero. En la Terminal: `atalaya mcps list|add|test|enable|disable|remove`.
- **Tareas** (vista Tareas). Programa encargos que corren solos (cada día, semana o tantas
  horas) y reacciones: cuando falla un despliegue o el sitio, una IA diagnostica la causa
  apenas la Vigía lo detecta y te avisa. Solo leen. En la Terminal: `atalaya tareas` y
  `atalaya reacciones`.
- **Comparar** (vista Agente). Marca dos a cuatro IA y haz la misma pregunta a todas: las
  respuestas quedan lado a lado, con lo que tardó y costó cada una. Con «Seguir con…»
  continúas con la que prefieras.
- **Tope por encargo** (vista Consumo). Lo máximo que puede gastar un solo encargo.
  Claude Code se detiene al llegar y los modelos por API acortan su respuesta. En la
  Terminal: `atalaya ai tope 0.5`.
- **Memoria** (vista Skills, memoria y MCP). Claude Code propone en el chat lo que vale
  la pena recordar de cada proyecto; tú lo apruebas, lo ajustas o lo descartas, y lo
  aprobado llega a todas las IA. También puedes escribirlo tú. En la Terminal:
  `atalaya memoria list|add|approve|discard`.
- **Comandos guardados.** En el chat, escribe `/` y elige una receta (`/explicar el
  login`, `/revisar-seguridad`…). Se crean y editan en la vista Skills, memoria y MCP, o
  con `atalaya comandos list|add|examples|remove`.
- **Skills e instrucciones** (vista Skills). Instala una skill desde una carpeta, un
  repo de git o importándola de las que ya tienes en Claude Code o Codex. Se instala
  apagada: revisa qué trae (Atalaya marca los programas que podría ejecutar) y actívala.
  Puedes limitarla a ciertos proyectos o a ciertas IA. Las instrucciones («usamos pnpm»,
  «responde en español») valen para todas las IA o para un proyecto. Todo llega a cada
  IA cuando trabaja: en el chat, en las mejoras, en los encargos y en la ronda, sin tocar
  la configuración de cada agente. Quitar una skill manda su carpeta a la Papelera. En la
  Terminal: `atalaya skills list|add|show|enable|disable|remove|instructions`.
- **Adjuntar archivos.** En el chat, pega una captura con ⌘V, arrastra archivos a la
  ventana o elígelos con 📎. Claude Code y los agentes los leen desde su ruta. Los
  modelos por API ven las imágenes y el texto de los archivos de texto. Para que lea un
  PDF, elige Claude Code. Quedan guardados en `adjuntos/`, dentro de la carpeta de
  Atalaya.
- **Mejoras y decisiones sin salir del chat.** La barra **Tablero**, arriba de la
  conversación, cuenta lo que espera algo de ti: decisiones pendientes, mejoras que te
  preguntan algo, en revisión, propuestas, aceptadas y en curso. Al tocar un grupo se
  abre la lista con los mismos botones de las vistas Mejoras y Decisiones: aceptar,
  mandar al agente, fusionar, marcar hecha, aprobar o descartar. «Hazla aquí» le pide a
  Claude Code que trabaje la mejora en la conversación, y «Preguntar» o «Pedir cambios»
  te deja el mensaje empezado. Cuando Claude Code toca una mejora o una decisión, su
  tarjeta aparece bajo ese paso, al día. Claude Code solo aprueba o descarta una decisión
  si se lo pides, y espera tu visto bueno.
- **Consumo** muestra quién hizo el trabajo del mes: qué porcentaje hizo cada IA, en qué
  tareas y si fue gratis, con tu cuenta o pagado por uso. También cuánto gastó cada una.
  Puedes fijar un presupuesto por conexión. Atalaya no inventa precios: si no conoce el
  de un modelo, muestra los tokens y te deja fijarlo.

## Tus datos

- Todo vive en tu Mac:
  - la base de Atalaya y los adjuntos del chat están en `~/Library/Application Support/Atalaya`;
  - las llaves, en el Llavero de macOS.
- Atalaya solo se comunica con los servicios que conectas tú.

## Si algo no funciona

En la Terminal:

```bash
atalaya doctor
```

Revisa permisos, herramientas, conexiones y quién está listo en cada rol de IA, y dice
cómo arreglar lo que falta.

## Desinstalar

1. Detén el vigía:

   ```bash
   atalaya service uninstall
   ```

2. Arrastra **Atalaya** de Aplicaciones a la papelera.
3. Quita el comando de la Terminal (pide tu contraseña):

   ```bash
   sudo rm /usr/local/bin/atalaya
   ```

Tus datos quedan en `~/Library/Application Support/Atalaya`, por si vuelves a instalarlo.
Las llaves quedan en el Llavero, con el nombre «Atalaya». Puedes borrar ambos a mano si no
lo vas a usar más.

## Qué nos sirve saber

- ¿Entendiste qué hace Atalaya al abrirla por primera vez?
- ¿La bienvenida encontró tus proyectos? ¿Faltó alguno o sobró alguno?
- ¿Pudiste conectar tus servicios? ¿Cuál faltó o costó?
- ¿Las alertas te parecieron útiles o ruido?
- Si probaste la IA: ¿qué agentes o modelos usaste? ¿Pudiste iniciar sesión desde
  Atalaya? ¿Se entendieron los roles? ¿El consumo cuadra con lo que ves en cada
  proveedor?
- ¿Qué fue lo más confuso? ¿Qué te gustaría que hiciera y no hace?
- Si algo falló, copia lo que dice `atalaya doctor` y cuéntanos qué estabas haciendo.
