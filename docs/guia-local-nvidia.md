# Guía local: VIctory en una RTX 3060 Ti con mando Switch Pro

Notas de la puesta en marcha en este equipo (Windows 11, RTX 3060 Ti 8 GB,
32 GB de RAM). Describe lo que se hizo, cómo se lanza, qué cambió en el
sistema y cómo deshacerlo. Es una guía de este PC, no del proyecto original.

## Ramas

Todas son locales; nada se subió a GitHub.

| Rama | Qué es |
|------|--------|
| `integration/all-prs` | Rama principal de trabajo: 299 PR abiertos de upstream, con los crates `victory-shader`, `victory-gpu`, `victory-loader`, `victory-game` y `apps/victory` tomados **enteros del fork** `fxceless/victory-fork`, y el resto de crates con los PR. Incluye el mando, los logs a archivo y el prewarm. |
| `integration/wave64` | Base estable: `main` más el PR #1589 (wave64 en GPU de 32 lanes). Sin los PR extra. |
| `input/switch-pro` | `wave64` más la lectura del mando Switch Pro. |
| `input/shaders` | `input/switch-pro` más el bundle #1638 (pila NVIDIA). |
| `fork/switch-pro` | El fork limpio más nuestros commits (worktree aparte). |

Punto de retorno de `integration/all-prs` antes de traer el fork: `9a82eb52`
(`git reset --hard 9a82eb52` lo deshace).

Por qué el fork entero y no una fusión por trozos: reescribe las mismas
funciones que los PR (`lower.rs`, `structure.rs`, `resource.rs`), y elegir
trozos de cada lado daba 42 errores de compilación en shaders. Sus crates son
coherentes entre sí y declara el mundo renderizándose en una RTX 4070.

## Cómo lanzarlo

```powershell
$env:VICTORY_LOGS = '1'
.\target\release\victory.exe run "<carpeta del juego>\game_scarlett_master.exe" --execute --prewarm
```

- `--prewarm` traduce todos los shaders de `shaders.rpf` a la caché antes de
  arrancar (unos 3 minutos la primera vez).
- `VICTORY_LOGS=1` muestra el log en una consola propia; `VICTORY_LOGS=0` lo
  apaga y oculta esa consola.
- Cada vez que se recompila `victory.exe` la caché de shaders deja de valer,
  porque su clave incluye la huella del ejecutable: hay que repetir el prewarm.

## Logs

- Se guardan siempre (salvo con `VICTORY_LOGS=0`) en
  `%LOCALAPPDATA%\victory\logs\victory-<segundos unix>-<pid>.log`, un archivo
  por proceso y los últimos 10. `VICTORY_LOG_DIR` cambia la carpeta.
- Un título que se relanza a sí mismo escribe su propio archivo sin pisar el
  anterior.
- Para revisar una ejecución, el archivo más reciente es el último por nombre.

## Prewarm

`victory prewarm [rutas] [--jobs N]` (y `run --prewarm`) traduce los
contenedores de shaders con todos los núcleos y los deja en la caché con la
misma clave que usa el juego. En `shaders.rpf` hay 26.503 contenedores; en
una prueba tardó 3:23, con 23.337 traducidos y 2.565 rechazados por el
traductor (los que aún no sabe hacer).

El progreso sale como líneas de log cada 2 segundos. No usa el título de la
consola: la secuencia que lo fija termina en BEL, que Windows reproduce como
alerta sonora.

No precompila los pipelines (PSO): dependen del estado de dibujo del juego y
se crean en ejecución.

## Mando Nintendo Switch Pro

Windows lo ve por Bluetooth pero no entrega pulsaciones a XInput, `winmm` ni
`Windows.Gaming.Input` hasta que un programa fija su modo de informes (Steam
lo hace). VIctory ahora lo abre por HID, le pide informes completos (`0x30`) y
los lee en un hilo propio. Código: `crates/victory-hle/src/modules/gameinput/switchpro.rs`.

- Ocupa el primer slot de mando que XInput y el DualSense dejen libre.
- Los botones van **por posición**: el de abajo del rombo es la A del juego.
  ZL y ZR son digitales (0 o 1). Menos es Vista y Más es Menú.
- Sin vibración. La calibración de los sticks es genérica (centro 2048,
  recorrido ±1500).
- Si el mando se duerme, el hilo lo recupera solo en cuanto vuelve.

## Cambios del sistema en este equipo

Hechos con permiso del usuario; hace falta reiniciar para que valgan.

1. **TDR de Windows.** En `HKLM\SYSTEM\CurrentControlSet\Control\GraphicsDrivers`:
   `TdrDelay = 30` y `TdrDdiDelay = 40` (antes sin definir, es decir 2 s y
   5 s). Evita que Windows reinicie el driver de vídeo cuando la GPU tarda en
   un envío largo. Para deshacerlo, borrar ambos valores.
2. **Archivo de paginación.** La gestión automática se desactivó y quedó:
   `D:\pagefile.sys` 32768–65536 MB y `C:\pagefile.sys` 8192–16384 MB (antes
   automático, unos 14 GB en `C:`). El juego reserva unos 24 GB de memoria
   comprometida; con el límite en 46 GB y 44,9 GB usados, el sistema se
   quedaba sin commit y el juego pasaba a 0 fps, con audio cortado y la ventana
   sin responder. Para volver a lo anterior: «Administrar automáticamente el
   tamaño del archivo de paginación» en Configuración avanzada del sistema.

## Estado y límites conocidos

- Menús a 30 fps. En el mundo, el fork declara 2,5–4 fps en una RTX 4070 y las
  zonas en sombra salen negras (falta la iluminación por trazado de rayos).
- Sobre unos 230 shaders del mundo no se traducen aún, sobre todo de compute:
  `v_readfirstlane` divergente, máscaras de lanes leídas como dato, ballot de
  64 lanes divergente, LDS en wave64 dividido.
- NVIDIA tiene waves de 32 lanes; los programas wave64 de la consola pasan por
  una traducción de 32 en 32 (`wave64_mode`).
- `victory-gpu` tiene un test que falla de forma intermitente (violación de
  acceso, 1 de cada 10 ejecuciones en la rama combinada); el fork limpio no lo
  mostró en 10 ejecuciones. Sin causa conocida.
- El proyecto dibuja solo con Direct3D 12; no hay backend Vulkan. Añadir uno
  supondría reescribir `victory-gpu` y emitir SPIR-V, y no arregla los fallos
  de traducción de shaders.

## Dependencias

Revisadas el 2026-11-25 contra crates.io. Aplicados los parches `anyhow`
1.0.105, `clap` 4.6.10 y `opus-pure` 0.2.5. `windows-core` queda en 0.62.2
(existe 0.100.0, que exige Rust 1.95 y va ligada a la crate `windows`, aún en
0.62.2). No se pasó una auditoría de seguridad (`cargo audit`).
