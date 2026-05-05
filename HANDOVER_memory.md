# Memory.md — Decisiones de arquitectura y hechos clave

## Decisiones de diseño tomadas

### Arquitectura general del fork

* De Ludusavi original mantenemos: GUI base (Iced 0.14), detección de paths de saves via manifest, escaneo de juegos del sistema (solo cuando el usuario lo pide), bridge de registro post-backup.
* De Ludusavi original eliminamos: tema claro por defecto (ahora Dark), check automático de actualizaciones (clase Modal::AppUpdate, campo release en cache y config, módulo metadata.rs completo), tabs Backup/Restore/Custom games del sidebar, sección Advanced del sidebar, selector de idioma y tema en Settings, redirects editor, backup exclusions editor.
* De EmuSync adaptamos: sistema de ZIP + rclone, daemon con file watcher y polling, device identity por UUID, conflict detection por timestamps, game-list.json compartido en cloud.

### Limpieza masiva de dead code del Ludusavi original (sesión abril 2026)

Se eliminó completamente todo el código del sistema de backup/restore original de Ludusavi que no se usaba en el fork:

Enums y variantes eliminados:

* RestorePhase — enum completo eliminado
* ValidatePhase — enum completo eliminado
* Operation::Restore y Operation::ValidateBackups — variantes eliminadas de todos los métodos
* Screen::Restore — variante eliminada
* ScrollSubject::Restore — variante eliminada
* GameAction — enum completo eliminado
* Message: eliminadas ~15 variantes (CancelOperation, Restore, ValidateBackups, RequestForceUpload, RequestForceDownload, ForceUploadGame, ForceDownloadGame, ToggleGameListEntryExpanded, SelectAllGames, DeselectAllGames, GameAction, ShowGameNotes, EditedBackupComment, FilterDuplicates, OpenUrlAndCloseModal, ConfirmSynchronizeCloud, ShowCustomGame, SelectedBackupToRestore, ToggleCustomGameExpanded)
* Modal: eliminadas ConfirmBackup, ConfirmRestore, GameNotes, BackupValidation, ConfirmForceUpload, ConfirmForceDownload
* CloudModalState::Initial eliminado
* Kind::GameNotes, Kind::BackupValidation, Kind::ConfirmBackup, Kind::ConfirmRestore, Kind::ConfirmForceUpload, Kind::ConfirmForceDownload eliminados
* UndoSubject: eliminadas ~15 variantes (RestoreSource, BackupSearchGameName, RestoreSearchGameName, CustomGamesSearchGameName, RedirectSource, RedirectTarget, CustomGameName, CustomGameAlias, CustomGameFile, CustomGameRegistry, CustomGameInstallDir, CustomGameWinePrefix, BackupFilterIgnoredPath, BackupFilterIgnoredRegistry, BackupComment)
* BrowseSubject: eliminadas RestoreSource, RedirectSource, RedirectTarget, CustomGameFile, BackupFilterIgnoredPath
* SaveKind::Backup eliminado

Métodos y funciones eliminados de app.rs:

* handle_restore — método completo (~200 líneas)
* handle_validation — método completo (~100 líneas)
* save_backup, customize_game, customize_game_as_alias, open_wiki, toggle_backup_comment_editor

Campos eliminados:

* backups_to_restore: HashMap<String, BackupId> de App struct
* backup_comments: HashMap<String, TextHistory> de TextHistories
* comment_editor: Option<...> de GameListEntry
* restore_search_game_name: TextHistory de TextHistories

Otros eliminados:

* Icon::Info, Icon::Comment, Icon::Edit, Icon::FastForward, Icon::Language, Icon::Lock, Icon::LockOpen, Icon::PlayCircleOutline, Icon::Refresh del enum Icon
* Método text_narrow de Icon
* ERROR_ICON constante de common.rs
* restore_scroll / RESTORE_SCROLL de widget.rs
* Método input() de TextHistories (solo existe input_small())
* path_appears_valid de shortcuts.rs
* apply_to_registry_path_field de shortcuts.rs
* Vistas Backup::view, Restore::view, CustomGames::view de screen.rs
* ~40 funciones de button.rs
* Muchas funciones de game_list.rs, search.rs, badge.rs, notification.rs, widget.rs, editor.rs

Nota importante: BackupPhase y handle_backup siguen vivos — se usan para poblar la tabla Games con el scan de juegos. Icon::Copy también vivo (se usa en file_tree.rs).

### Modelo de sincronización (Save Modes)

Cada juego tiene un modo y un flag auto_sync. Los modos son cuatro:

* NONE (default): juego ignorado completamente por el daemon.
* LOCAL: backup a un ZIP en config.backup.path. Nunca toca el cloud.
* CLOUD: backup unidireccional al cloud (solo sube, nunca baja automáticamente).
* SYNC: bidireccional (sube si local más nuevo, baja si cloud más nuevo). Auto sync siempre ON implícitamente.

auto_sync controla si el daemon actúa automáticamente (file watcher) o si el usuario tiene que dispararlo manualmente con el botón Backup/Restore.

### Botones en GameDetail header tras simplificación

Decidimos quitar Force upload/download de CLOUD y SYNC porque en el código real:

* SyncBackupGame y ForceUploadGame en modo CLOUD llaman ambos a upload_game directo sin comparar timestamps.
* SyncRestoreGame y ForceDownloadGame son literalmente el mismo código (download_game).

La distinción Backup/Force era legacy sin efecto. Simplificado a:

| MODO | AUTO SYNC | BOTONES |
| :-: | :-: | :-: |
| None | - | (Ninguno, solo ← Back) |
| Local | Off | Backup, Restore |
| Local | On | Sync Now, Backup, Restore |
| Cloud | Off | Backup, Restore |
| Cloud | On | Sync Now, Backup, Restore |
| Sync | (Siempre ON) | Sync Now, Backup, Restore |

Los handlers ForceUploadGame/ForceDownloadGame han sido eliminados completamente del código.

### Sistema tipográfico 13/12/11

Decidido como estándar global del programa:

* 13 → info importante (lo que comunica)
* 12 → info secundaria (contexto)
* 11 → info terciaria (metadatos, headers de tabla, labels auxiliares)
* 15/16 → super-headers fuera del sistema

### Tema oscuro por defecto

Theme::Dark es ahora el default en lugar de Theme::Light. Cambio hecho moviendo el atributo #[default] en el enum.

### FILES section oculta

Los toggles (checkboxes) del FileTree no funcionan con el sistema de sync actual:

* create_zip_from_folder en sync/operations.rs usa walkdir sin consultar config.backup.toggled_paths.
* upload_game llama a esa función → toggles ignorados.
* extract_zip_to_directory (restore) restaura todo sin filtrar.

Decisión: esconder la sección FILES hasta que se implemente filtrado real en el daemon. El .push(files_card) está comentado en el Column principal. Descomentar una línea lo re-activa.

### Scan bajo demanda en GameDetail

El scan automático al entrar a GameDetail se eliminó (causaba parpadeo de la barra de progreso). Comportamiento actual: al expandir FILES, si el juego tiene scanned == true en backup_screen.log.entries, se expande el tree sin escanear. Si no, lanza scan del juego individual (no de todo el manifest).

### Menú ⋯ popup en Games

Ancho del dropdown reducido de 160px a 120px en popup_menu.rs. El 160 era para acomodar "Force upload/Force download". Ahora la opción más larga es "Sync now".

### Modales FTP/SMB/WebDAV funcionan con nuestro sistema

Todo el sync pasa por rclone. Si rclone puede hablar con un remote, nuestro pipeline de ZIP+upload/download funciona. Google Drive y OneDrive están confirmados.

### Alineación de pick_lists con inputs

Los pick_lists de Settings llevan .text_size(12).padding([5, 5]) para coincidir con los inputs input_small.

## Hechos clave del código

### El daemon en LOCAL sin entrada en game-list

Para juegos en LOCAL, no hace falta entrada en game-list.json del cloud. El daemon resuelve path via manifest y actúa sobre el ZIP local.

### managed_games filtrado

managed_games solo incluye juegos que realmente entran en watched_paths (SYNC, LOCAL+autoSync, CLOUD+autoSync). LOCAL+autoSync OFF y NONE no entran.

### Last Synced solo tiene sentido en SYNC

Las columnas "Last Synced From" y "Last Synced" de la tabla Games solo muestran datos en modo SYNC. Para CLOUD/LOCAL/NONE muestran "—".

### Status calculado en GUI para LOCAL/NONE

* LOCAL: compara mtime del ZIP local vs mtime de saves.
* NONE: siempre "not_managed".
* CLOUD y SYNC: el status lo escribe el daemon en daemon-status.json.

### AllDevices filtra por SYNC local

La pantalla AllDevices solo muestra devices para juegos en modo SYNC local.

### Sistema check de actualizaciones eliminado completamente

Borrado: src/metadata.rs, Modal::AppUpdate, Message::CheckAppRelease, campo release en cache y config, pub mod metadata en lib.rs, etc.

### Toggles del tree NO están wired al daemon

create_zip_from_folder usa walkdir sin filtrar. resolve_game_path_from_manifest pasa BackupFilter::default(), ToggledPaths::default(), ToggledRegistry::default(). Los toggles se guardan en config pero nadie los lee en el path de sync.

### Operation::Restore eliminado

handle_restore fue eliminado completamente. Operation::Restore ya no existe. Solo existen Backup, Cloud, Idle.

### ValidateBackups eliminado

handle_validation fue eliminado completamente. Operation::ValidateBackups ya no existe. La validación de backups del Ludusavi original no forma parte de este fork.

## Versiones y configuración

* Iced: 0.14
* Rust edition: 2021
* rclone: cualquier versión reciente con soporte OAuth/FTP/SMB/WebDAV
* CI: GitHub Actions compila para Windows x64/x32, Linux, Mac en cada push/PR
* OSes objetivo: Windows 10/11 nativo, SteamOS 3.x (Steam Deck)

## Rutas importantes en producción

Windows

* Config: C:\Users\<user>\AppData\Roaming\ludusavi\
* Logs: %APPDATA%\ludusavi\daemon.log
* Tarea programada: nombre "LudusaviDaemon" sin admin

Linux/SteamOS

* Config: ~/.config/ludusavi/
* Logs: ~/.config/ludusavi/daemon.log
* Servicio systemd user: ~/.config/systemd/user/ludusavi-daemon.service


## Tanda 9 parte 2 + refactor de ScanInfo (mayo 2026)

Continuación de la limpieza zombi: el sistema de backup-layout heredado de upstream (full + diff backups, validate, restore, retention, mapping.yaml, GameLayout entera) llevaba muerto en runtime desde la Tanda 9 parte 1 pero sobrevivía en compilación porque ScanInfo y scan_game_for_backup lo referenciaban como tipo dormante.

Cambios en el código:

* src/scan/preview.rs: eliminados los campos available_backups: Vec<Backup>, backup: Option<Backup> y has_backups: bool de ScanInfo. Eran siempre vec![]/None/false en runtime.
* src/scan.rs: eliminado el parámetro previous: Option<&LatestBackup> de scan_game_for_backup. Pasábamos None desde todos los call sites.
* src/scan/preview.rs: scan_kind() simplificado a ScanKind::Backup constante (el fork no hace restore scans). is_downgraded_backup y is_downgraded_restore borrados (sin callers).
* src/scan/layout.rs: 475 → 58 líneas. Solo sobreviven escape_folder_name (usado por manifest.rs para nombres de manifests secundarios) y un BackupLayout shell con new + restorable_game_set que devuelve siempre un set vacío (TitleFinder::new espera ese parámetro). Eliminados: GameLayout entera + impl, LatestBackup, Backup enum, FullBackup, DifferentialBackup, BackupInclusion, BackupKind, IndividualMapping*.
* src/sync/bridge.rs: ARCHIVO ELIMINADO. La función register_game_after_backup no tenía callers vivos en runtime — los 4 handlers de gui/app.rs que suben juegos (Add Custom Game, SyncBackupGame, Sync now, conflict resolution) llaman directamente a upload_game con su propia secuencia read_game_list_from_cloud + upsert_game + upload_game.
* src/gui/app.rs: bloque if !preview { back_up(...) } eliminado. El campo syncable_games de Operation::Backup y los métodos add_syncable_game/syncable_games() también eliminados (sin callers tras la limpieza).
* src/gui/game_list.rs: campo game_layout: Option<GameLayout> de GameListEntry eliminado (siempre era None en runtime).

Bugs reales encontrados y arreglados durante el refactor:

* src/sync/sync_config.rs: GameSyncConfig::default() devolvía auto_sync = false mientras que el default de serde para JSON sin el campo era true. Inconsistencia visible en la UI: tras seleccionar un modo LOCAL/CLOUD por primera vez para un juego, el toggle "Auto Sync" aparecía ON al principio y se ponía OFF al cambiar de modo. Fix: impl Default manual unificado a true.
* src/sync/operations.rs: read_game_list_from_cloud y write_game_list_to_cloud usaban /tmp/ludusavi-game-list-temp.json hardcoded. Carrera real entre operaciones concurrentes del daemon (el segundo write podía borrar el temp del primero antes de que rclone terminara, error "directory not found"). Fix: tempfile::NamedTempFile con prefix/suffix únicos. tempfile movido de dev-deps a deps.
* src/sync/daemon.rs: write_sync_status_with_errors sobreescribía daemon-status.json con solo {"games": ...}, perdiendo el flag rclone_missing que write_rclone_missing_flag había escrito al detectar rclone caído. Fix: leer JSON existente, mergear solo la clave "games", preservar el resto.

Refactor de testabilidad:

* src/sync/daemon.rs: auto_register_paths ahora recibe el Manifest como parámetro en lugar de cargarlo internamente con Manifest::load(). Permite inyectar manifests controlados con Manifest::load_from_string en tests sin necesidad de manipular el filesystem global.

Suite de tests pasó de 126 → 269 tests:

* conflict.rs: 13 (determine_sync_type, todas las ramas + edge case nanos Windows)
* game_list.rs: 13 (round-trip JSON, merge, device_names, backwards-compat)
* sync_config.rs: 14 (round-trip, modificaciones, regresión del bug auto_sync, backwards-compat)
* device.rs: 3 (UUID v4 shape, unicidad, round-trip JSON)
* operations.rs: 79 (puro: classify_error 13 patrones; FS: ZIP round-trip, preservación mtime, atomic swap, scan; rclone harness :local: con upload_game/download_game/lsjson/deletefile/game-list; safety_backup edge cases; E2E PC↔Deck con SimulatedDevice)
* daemon.rs: 24 (normalize_path, save_last_mod_time, write_rclone_missing_flag, write_game_list_local merge, calculate_game_status 6 ramas, write_sync_status_with_errors + regresión rclone_missing, auto_register_paths con manifest inyectado: cloud vacío, device_name update, skip cuando ya OK, juegos no en manifest, solo SYNC procesado, registro real con xdgData)
* tests/daemon_smoke.rs: 4 (binario arranca, exits limpio sin cloud, SIGTERM con cloud configurado, E2E watcher → debounce → upload del binario real)

Total tests del fork (excluyendo upstream-heredados): 142.
Tiempo suite completa: ~20s (la mayoría es el smoke E2E del daemon, que necesita esperar el debounce de 10s).

Pendiente (no cubierto):

* Tests E2E del worker loop para los otros 3 escenarios: cloud polling → download (test ~35s), reacción a cambios en sync-games.json (test ~12s), retry cuando cloud vacío (test ~35s). Comparten infraestructura con el test ya existente; dejado por coste/lentitud.
* GUI handlers en src/gui/app.rs (4000+ líneas, cero tests). Requiere refactor mayor para extraer la lógica de los handlers de Iced.
* src/sync/operations.rs::create_safety_backup límite de 500MB no testeado (requeriría crear un dir > 500MB en tempdir, lento).

CI:

* .github/workflows/main.yaml: añadido rclone install al job test para los 3 OS (Linux/Windows/macOS). Necesario para que los tests rclone (Fases 3-4 + auto_register_paths + smoke daemon E2E) pasen en CI.

