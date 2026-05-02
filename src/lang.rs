use crate::{
    prelude::{CommandError, Error, StrictPath, VARIANT},
    resource::{
        config::{BackupFormat, CustomGameKind, Root, SortKey, Theme, ZipCompression},
        manifest::Store,
    },
    scan::{game_filter, OperationStatus},
};

const PATH: &str = "path";
const PROCESSED_SIZE: &str = "processed-size";
const TOTAL_SIZE: &str = "total-size";
const COMMAND: &str = "command";
const CODE: &str = "code";
const MESSAGE: &str = "message";
const APP: &str = "app";
const GAME: &str = "game";
const VERSION: &str = "version";

pub const TRANSLATOR: Translator = Translator {};
pub const ADD_SYMBOL: &str = "+";
pub const CHANGE_SYMBOL: &str = "Δ";
pub const REMOVAL_SYMBOL: &str = "x";

fn title_case(text: &str) -> String {
    let lowercase = text.to_lowercase();
    let mut chars = lowercase.chars();
    match chars.next() {
        None => lowercase,
        Some(char) => format!("{}{}", char.to_uppercase(), chars.as_str()),
    }
}

#[derive(Default)]
pub struct LangArgs {
    items: Vec<(&'static str, String)>,
}

impl LangArgs {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&mut self, key: &'static str, value: impl ToString) {
        self.items.push((key, value.to_string()));
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Translator {}

fn translate(id: &str) -> String {
    let s: &'static str = match id {
        "ludusavi" => "Ludusavi",
        "game-name" => "Name",
        "total-games" => "Games",
        "status" => "Status",
        "badge-failed" => "FAILED",
        "badge-duplicates" => "DUPLICATES",
        "badge-duplicated" => "DUPLICATED",
        "badge-ignored" => "IGNORED",
        "some-entries-failed" => "Some entries failed to process; look for [FAILED] in the output for details. Double check whether you can access those files or whether their paths are very long.",
        "button-backup" => "Back up",
        "button-restore" => "Restore",
        "button-nav-backup" => "BACKUP MODE",
        "button-nav-restore" => "RESTORE MODE",
        "button-nav-custom-games" => "CUSTOM GAMES",
        "button-nav-other" => "OTHER",
        "button-add-game" => "Add game",
        "button-continue" => "Continue",
        "button-cancel" => "Cancel",
        "button-cancelling" => "Cancelling...",
        "button-okay" => "Okay",
        "button-select-all" => "Select all",
        "button-deselect-all" => "Deselect all",
        "button-enable-all" => "Enable all",
        "button-disable-all" => "Disable all",
        "button-customize" => "Customize",
        "button-exit" => "Exit",
        "button-comment" => "Comment",
        "button-lock" => "Lock",
        "button-unlock" => "Unlock",
        "button-validate" => "Validate",
        "button-override-manifest" => "Override manifest",
        "button-extend-manifest" => "Extend manifest",
        "button-sort" => "Sort",
        "button-download" => "Download",
        "button-upload" => "Upload",
        "button-ignore" => "Ignore",
        "no-roots-are-configured" => "Add some roots to back up even more data.",
        "config-is-invalid" => "Error: The config file is invalid.",
        "manifest-is-invalid" => "Error: The manifest file is invalid.",
        "manifest-cannot-be-updated" => "Error: Unable to check for an update to the manifest file. Is your Internet connection down?",
        "registry-issue" => "Error: Some registry entries were skipped.",
        "unable-to-browse-file-system" => "Error: Unable to browse on your system.",
        "unable-to-open-directory" => "Error: Unable to open directory:",
        "unable-to-open-url" => "Error: Unable to open URL:",
        "unable-to-configure-cloud" => "Unable to configure cloud.",
        "cloud-synchronize-conflict" => "Your local and cloud backups are in conflict. Perform an upload or download to resolve this.",
        "field-backup-target" => "Back up to:",
        "field-restore-source" => "Restore from:",
        "field-custom-files" => "Paths:",
        "field-custom-registry" => "Registry:",
        "field-sort" => "Sort:",
        "field-roots" => "Roots:",
        "field-backup-excluded-items" => "Backup exclusions:",
        "field-backup-format" => "Format:",
        "field-backup-compression" => "Compression:",
        "field-backup-compression-level" => "Level:",
        "label-manifest" => "Manifest",
        "label-checked" => "Checked",
        "label-updated" => "Updated",
        "label-new" => "New",
        "label-removed" => "Removed",
        "label-comment" => "Comment",
        "label-unchanged" => "Unchanged",
        "label-backup" => "Backup",
        "label-scan" => "Scan",
        "label-filter" => "Filter",
        "label-unique" => "Unique",
        "label-complete" => "Complete",
        "label-partial" => "Partial",
        "label-enabled" => "Enabled",
        "label-disabled" => "Disabled",
        "label-threads" => "Threads",
        "label-remote" => "Remote",
        "label-remote-name" => "Remote name",
        "label-folder" => "Folder",
        "label-executable" => "Executable",
        "label-arguments" => "Arguments",
        "label-url" => "URL",
        "label-host" => "Host",
        "label-port" => "Port",
        "label-username" => "Username",
        "label-password" => "Password",
        "label-provider" => "Provider",
        "label-custom" => "Custom",
        "label-none" => "None",
        "label-unscanned" => "Unscanned",
        "label-file" => "File",
        "label-game" => "Game",
        "label-alias" => "Alias",
        "label-original-name" => "Original name",
        "label-source" => "Source",
        "label-primary-manifest" => "Primary manifest",
        "label-integration" => "Integration",
        "label-installed-name" => "Installed name",
        "file-size" => "Size",
        "store-ea" => "EA",
        "store-epic" => "Epic",
        "store-gog" => "GOG",
        "store-gog-galaxy" => "GOG Galaxy",
        "store-heroic" => "Heroic",
        "store-legendary" => "Legendary",
        "store-lutris" => "Lutris",
        "store-microsoft" => "Microsoft",
        "store-origin" => "Origin",
        "store-prime" => "Prime Gaming",
        "store-steam" => "Steam",
        "store-uplay" => "Uplay",
        "store-other-home" => "Home folder",
        "store-other-wine" => "Wine prefix",
        "store-other-windows" => "Windows drive",
        "store-other-linux" => "Linux drive",
        "store-other-mac" => "Mac drive",
        "store-other" => "Other",
        "backup-format-simple" => "Simple",
        "backup-format-zip" => "Zip",
        "compression-none" => "None",
        "compression-deflate" => "Deflate",
        "compression-bzip2" => "Bzip2",
        "compression-zstd" => "Zstd",
        "theme" => "Theme",
        "theme-light" => "Light",
        "theme-dark" => "Dark",
        "show-disabled-games" => "Show disabled games",
        "show-unchanged-games" => "Show unchanged games",
        "show-unscanned-games" => "Show unscanned games",
        "override-max-threads" => "Override max threads",
        "synchronize-automatically" => "Synchronize automatically",
        "prefer-alias-display" => "Display alias instead of original name",
        "skip-unconstructive-backups" => "Skip backup when data would be removed, but not added or updated",
        "explanation-for-exclude-store-screenshots" => "In backups, exclude store-specific screenshots",
        "explanation-for-exclude-cloud-games" => "Do not back up games with cloud support on these platforms",
        "consider-doing-a-preview" => "If you haven't already, consider doing a preview first so that there are no surprises.",
        "confirm-restore" => "Are you sure you want to proceed with the restoration?\nThis will overwrite any current files with the backups from here:",
        "confirm-add-missing-roots" => "Add these roots?",
        "no-missing-roots" => "No additional roots found.",
        "preparing-backup-target" => "Preparing backup directory...",
        "updating-manifest" => "Updating manifest...",
        "backups-are-valid" => "Your backups are valid.",
        "backups-are-invalid" => "These games' backups appear to be invalid.\nDo you want to create new full backups for these games?",
        "saves-found" => "Save data found.",
        "no-saves-found" => "No save data found.",
        "suffix-no-confirmation" => "no confirmation",
        "suffix-restart-required" => "restart required",
        "cloud-not-configured" => "Cloud backups are disabled because no cloud system is configured.",
        "cloud-path-invalid" => "Cloud backups are disabled because the backup path is invalid.",
        "game-is-unrecognized" => "Ludusavi does not recognize this game.",
        "game-has-nothing-to-restore" => "This game does not have a backup to restore.",
        "launch-game-after-error" => "Launch the game anyway?",
        "game-did-not-launch" => "Game failed to launch.",
        "backup-is-newer-than-current-data" => "The existing backup is newer than the current data.",
        "backup-is-older-than-current-data" => "The existing backup is older than the current data.",
        "new-version-check" => "Check for application updates automatically",
        "custom-game-will-override" => "This custom game overrides a manifest entry",
        "custom-game-will-extend" => "This custom game extends a manifest entry",
        "operation-will-only-include-listed-games" => "This will only process the games that are currently listed",
        _ => return format!("missing-translation={id}"),
    };
    s.to_string()
}

fn translate_args(id: &str, args: &LangArgs) -> String {
    let template: &'static str = match id {
        "badge-redirected-from" => "FROM: {$path}",
        "badge-redirecting-to" => "TO: {$path}",
        "button-get-app" => "Get {$app}",
        "command-unlaunched" => "Command did not launch: {$command}",
        "command-terminated" => "Command terminated abruptly: {$command}",
        "command-failed" => "Command failed with code {$code}: {$command}",
        "processed-size-subset" => "{$processed-size} of {$total-size}",
        "cannot-prepare-backup-target" => "Error: Unable to prepare backup target (either creating or emptying the folder). If you have the folder open in your file browser, try closing it: {$path}",
        "restoration-source-is-invalid" => "Error: The restoration source is invalid (either doesn't exist or isn't a directory). Please double check the location: {$path}",
        "prefix-error" => "Error: {$message}",
        "prefix-warning" => "Warning: {$message}",
        "cloud-app-unavailable" => "Cloud backups are disabled because {$app} is not available.",
        "back-up-specific-game.confirm" => "Back up save data for {$game}?",
        "back-up-specific-game.failed" => "Failed to back up save data for {$game}",
        "restore-specific-game.confirm" => "Restore save data for {$game}?",
        "restore-specific-game.failed" => "Failed to restore save data for {$game}",
        "new-version-available" => "An application update is available: {$version}. Would you like to view the release notes?",
        _ => return format!("missing-translation-args={id}"),
    };

    let mut result = template.to_string();
    for (key, value) in &args.items {
        let placeholder = format!("{{${}}}", key);
        result = result.replace(&placeholder, value);
    }
    result
}

impl Translator {
    pub fn app_name(&self) -> String {
        translate("ludusavi")
    }

    pub fn window_title(&self) -> String {
        let name = self.app_name();
        match VARIANT {
            Some(variant) => format!("{} v{} ({})", name, *crate::prelude::VERSION, variant),
            None => format!("{} v{}", name, *crate::prelude::VERSION),
        }
    }

    pub fn pcgamingwiki(&self) -> String {
        "PCGamingWiki".to_string()
    }

    pub fn comment_button(&self) -> String {
        translate("button-comment")
    }

    pub fn lock_button(&self) -> String {
        translate("button-lock")
    }

    pub fn unlock_button(&self) -> String {
        translate("button-unlock")
    }

    pub fn handle_error(&self, error: &Error) -> String {
        match error {
            Error::ConfigInvalid { why } => self.config_is_invalid(why),
            Error::ManifestInvalid { why, identifier } => self.manifest_is_invalid(why, identifier.as_deref()),
            Error::ManifestCannotBeUpdated { identifier } => self.manifest_cannot_be_updated(identifier.as_deref()),
            Error::NoSaveDataFound => self.notify_single_game_status(false),
            Error::GameIsUnrecognized => self.game_is_unrecognized(),
            Error::SomeEntriesFailed => self.some_entries_failed(),
            Error::CannotPrepareBackupTarget { path } => self.cannot_prepare_backup_target(path),
            Error::RestorationSourceInvalid { path } => self.restoration_source_is_invalid(path),
            Error::RegistryIssue => self.registry_issue(),
            Error::UnableToOpenDir(path) => self.unable_to_open_dir(path),
            Error::UnableToOpenUrl(url) => self.unable_to_open_url(url),
            Error::RcloneUnavailable => self.rclone_unavailable(),
            Error::CloudNotConfigured => self.cloud_not_configured(),
            Error::CloudPathInvalid => self.cloud_path_invalid(),
            Error::UnableToConfigureCloud(error) => {
                format!(
                    "{}\n\n{}",
                    self.prefix_error(&self.unable_to_configure_cloud()),
                    self.handle_command_error(error)
                )
            }
            Error::CloudConflict => TRANSLATOR.prefix_error(&TRANSLATOR.cloud_synchronize_conflict()),
            Error::GameDidNotLaunch { why } => format!("{}\n\n{}", self.game_did_not_launch(), self.prefix_error(why)),
        }
    }

    fn handle_command_error(&self, error: &CommandError) -> String {
        let mut args = LangArgs::new();
        args.set(COMMAND, error.command());
        match error {
            CommandError::Launched { raw, .. } => {
                format!("{}\n\n{}", translate_args("command-unlaunched", &args), raw)
            }
            CommandError::Terminated { .. } => translate_args("command-terminated", &args),
            CommandError::Exited {
                code, stdout, stderr, ..
            } => {
                args.set(CODE, code);
                let mut out = translate_args("command-failed", &args);

                if let Some(stdout) = stdout {
                    out.push_str("\n\n");
                    out.push_str(stdout);
                }

                if let Some(stderr) = stderr {
                    out.push_str("\n\n");
                    out.push_str(stderr);
                }

                out
            }
        }
    }

    pub fn cloud_not_configured(&self) -> String {
        translate("cloud-not-configured")
    }

    pub fn cloud_path_invalid(&self) -> String {
        translate("cloud-path-invalid")
    }

    pub fn some_entries_failed(&self) -> String {
        translate("some-entries-failed")
    }

    fn label(&self, text: &str) -> String {
        format!("[{text}]")
    }

    pub fn label_failed(&self) -> String {
        self.label(&self.badge_failed())
    }

    pub fn label_duplicates(&self) -> String {
        self.label(&self.badge_duplicates())
    }

    pub fn label_duplicated(&self) -> String {
        self.label(&self.badge_duplicated())
    }

    pub fn label_ignored(&self) -> String {
        self.label(&self.badge_ignored())
    }

    pub fn field(&self, text: &str) -> String {
        format!("{text}:")
    }

    pub fn field_theme(&self) -> String {
        self.field(&translate("theme"))
    }

    pub fn badge_failed(&self) -> String {
        translate("badge-failed")
    }

    pub fn badge_duplicates(&self) -> String {
        translate("badge-duplicates")
    }

    pub fn badge_duplicated(&self) -> String {
        translate("badge-duplicated")
    }

    pub fn badge_ignored(&self) -> String {
        translate("badge-ignored")
    }

    pub fn badge_redirected_from(&self, original: &StrictPath) -> String {
        let mut args = LangArgs::new();
        args.set(PATH, original.render());
        translate_args("badge-redirected-from", &args)
    }

    pub fn badge_redirecting_to(&self, path: &StrictPath) -> String {
        let mut args = LangArgs::new();
        args.set(PATH, path.render());
        translate_args("badge-redirecting-to", &args)
    }


    pub fn backup_button(&self) -> String {
        translate("button-backup")
    }

    pub fn backup_button_no_confirmation(&self) -> String {
        format!("{} ({})", self.backup_button(), self.suffix_no_confirmation())
    }

    pub fn restore_button(&self) -> String {
        translate("button-restore")
    }

    pub fn restore_button_no_confirmation(&self) -> String {
        format!("{} ({})", self.restore_button(), self.suffix_no_confirmation())
    }

    pub fn nav_backup_button(&self) -> String {
        translate("button-nav-backup")
    }

    pub fn nav_restore_button(&self) -> String {
        translate("button-nav-restore")
    }

    pub fn nav_custom_games_button(&self) -> String {
        translate("button-nav-custom-games")
    }

    pub fn custom_games_label(&self) -> String {
        title_case(&self.nav_custom_games_button())
    }

    pub fn nav_other_button(&self) -> String {
        translate("button-nav-other")
    }

    pub fn customize_button(&self) -> String {
        translate("button-customize")
    }

    pub fn no_missing_roots(&self) -> String {
        translate("no-missing-roots")
    }

    pub fn updating_manifest(&self) -> String {
        translate("updating-manifest")
    }

    pub fn backups_are_valid(&self) -> String {
        translate("backups-are-valid")
    }

    pub fn backups_are_invalid(&self) -> String {
        translate("backups-are-invalid")
    }

    pub fn confirm_add_missing_roots(&self, roots: &[Root]) -> String {
        use std::fmt::Write;
        let mut msg = translate("confirm-add-missing-roots") + "\n";

        for root in roots {
            let path2 = match &root {
                Root::Lutris(root) => root
                    .database
                    .as_ref()
                    .map(|x| format!(" + {}", x.render()))
                    .unwrap_or_default(),
                _ => "".to_string(),
            };
            let _ = &write!(
                msg,
                "\n[{}] {} {}",
                self.store(&root.store()),
                root.path().render(),
                path2
            );
        }

        msg
    }

    pub fn add_game_button(&self) -> String {
        translate("button-add-game")
    }

    pub fn continue_button(&self) -> String {
        translate("button-continue")
    }

    pub fn cancel_button(&self) -> String {
        translate("button-cancel")
    }

    pub fn cancelling_button(&self) -> String {
        translate("button-cancelling")
    }

    pub fn okay_button(&self) -> String {
        translate("button-okay")
    }

    #[allow(unused)]
    pub fn select_all_button(&self) -> String {
        translate("button-select-all")
    }

    #[allow(unused)]
    pub fn deselect_all_button(&self) -> String {
        translate("button-deselect-all")
    }

    pub fn enable_all_button(&self) -> String {
        translate("button-enable-all")
    }

    pub fn disable_all_button(&self) -> String {
        translate("button-disable-all")
    }

    pub fn exit_button(&self) -> String {
        translate("button-exit")
    }

    pub fn get_rclone_button(&self) -> String {
        let mut args = LangArgs::new();
        args.set(APP, "Rclone");
        translate_args("button-get-app", &args)
    }

    pub fn validate_button(&self) -> String {
        translate("button-validate")
    }

    pub fn override_manifest_button(&self) -> String {
        translate("button-override-manifest")
    }

    pub fn extend_manifest_button(&self) -> String {
        translate("button-extend-manifest")
    }

    pub fn sort_button(&self) -> String {
        translate("button-sort")
    }

    pub fn download_button(&self) -> String {
        translate("button-download")
    }

    pub fn upload_button(&self) -> String {
        translate("button-upload")
    }

    pub fn ignore_button(&self) -> String {
        translate("button-ignore")
    }

    pub fn no_roots_are_configured(&self) -> String {
        translate("no-roots-are-configured")
    }

    pub fn config_is_invalid(&self, why: &str) -> String {
        format!("{}\n{}", translate("config-is-invalid"), why)
    }

    pub fn manifest_is_invalid(&self, why: &str, identifier: Option<&str>) -> String {
        let message = translate("manifest-is-invalid");
        let identifier = identifier.map(|x| format!(" ({x})")).unwrap_or("".to_string());
        format!("{message}{identifier}\n{why}")
    }

    pub fn manifest_cannot_be_updated(&self, identifier: Option<&str>) -> String {
        let message = translate("manifest-cannot-be-updated");
        let identifier = identifier.map(|x| format!(" ({x})")).unwrap_or("".to_string());
        format!("{message}{identifier}")
    }

    pub fn cannot_prepare_backup_target(&self, target: &StrictPath) -> String {
        let mut args = LangArgs::new();
        args.set(PATH, target.render());
        translate_args("cannot-prepare-backup-target", &args)
    }

    pub fn restoration_source_is_invalid(&self, source: &StrictPath) -> String {
        let mut args = LangArgs::new();
        args.set(PATH, source.render());
        translate_args("restoration-source-is-invalid", &args)
    }

    pub fn registry_issue(&self) -> String {
        translate("registry-issue")
    }

    #[allow(unused)]
    pub fn unable_to_browse_file_system(&self) -> String {
        translate("unable-to-browse-file-system")
    }

    pub fn unable_to_open_dir(&self, path: &StrictPath) -> String {
        format!("{}\n\n{}", translate("unable-to-open-directory"), path.resolve())
    }

    pub fn unable_to_open_url(&self, url: &str) -> String {
        format!("{}\n\n{}", translate("unable-to-open-url"), url)
    }

    pub fn unable_to_configure_cloud(&self) -> String {
        translate("unable-to-configure-cloud")
    }

    pub fn cloud_synchronize_conflict(&self) -> String {
        translate("cloud-synchronize-conflict")
    }

    pub fn adjusted_size(&self, bytes: u64) -> String {
        let byte = byte_unit::Byte::from(bytes);
        let adjusted_byte = byte.get_appropriate_unit(byte_unit::UnitType::Binary);
        format!("{adjusted_byte:.2}")
    }

    pub fn processed_games(&self, status: &OperationStatus) -> String {
        let n = status.total_games;
        let unit = if n == 1 { "game" } else { "games" };
        if status.processed_all_games() {
            format!("{n} {unit}")
        } else {
            format!("{} of {} {}", status.processed_games, n, unit)
        }
    }

    pub fn processed_bytes(&self, status: &OperationStatus) -> String {
        if status.processed_all_bytes() {
            self.adjusted_size(status.total_bytes)
        } else {
            let mut args = LangArgs::new();
            args.set(TOTAL_SIZE, self.adjusted_size(status.total_bytes));
            args.set(PROCESSED_SIZE, self.adjusted_size(status.processed_bytes));
            translate_args("processed-size-subset", &args)
        }
    }

    pub fn processed_subset(&self, total: usize, processed: usize) -> String {
        let mut args = LangArgs::new();
        args.set(TOTAL_SIZE, total as u64);
        args.set(PROCESSED_SIZE, processed as u64);
        translate_args("processed-size-subset", &args)
    }

    pub fn backup_target_label(&self) -> String {
        translate("field-backup-target")
    }

    pub fn restore_source_label(&self) -> String {
        translate("field-restore-source")
    }

    pub fn custom_files_label(&self) -> String {
        translate("field-custom-files")
    }

    pub fn custom_registry_label(&self) -> String {
        translate("field-custom-registry")
    }

    pub fn custom_installed_name_label(&self) -> String {
        translate("label-installed-name")
    }

    pub fn sort_label(&self) -> String {
        translate("field-sort")
    }

    pub fn store(&self, store: &Store) -> String {
        translate(match store {
            Store::Ea => "store-ea",
            Store::Epic => "store-epic",
            Store::Gog => "store-gog",
            Store::GogGalaxy => "store-gog-galaxy",
            Store::Heroic => "store-heroic",
            Store::Legendary => "store-legendary",
            Store::Lutris => "store-lutris",
            Store::Microsoft => "store-microsoft",
            Store::Origin => "store-origin",
            Store::Prime => "store-prime",
            Store::Steam => "store-steam",
            Store::Uplay => "store-uplay",
            Store::OtherHome => "store-other-home",
            Store::OtherWine => "store-other-wine",
            Store::OtherWindows => "store-other-windows",
            Store::OtherLinux => "store-other-linux",
            Store::OtherMac => "store-other-mac",
            Store::Other => "store-other",
        })
    }

    pub fn sort_key(&self, key: &SortKey) -> String {
        translate(match key {
            SortKey::Name => "game-name",
            SortKey::Size => "file-size",
            SortKey::Status => "status",
        })
    }

    pub fn filter_uniqueness(&self, filter: game_filter::Uniqueness) -> String {
        match filter {
            game_filter::Uniqueness::Unique => translate("label-unique"),
            game_filter::Uniqueness::Duplicate => title_case(&self.badge_duplicated()),
        }
    }

    pub fn filter_completeness(&self, filter: game_filter::Completeness) -> String {
        translate(match filter {
            game_filter::Completeness::Complete => "label-complete",
            game_filter::Completeness::Partial => "label-partial",
        })
    }

    pub fn filter_enablement(&self, filter: game_filter::Enablement) -> String {
        translate(match filter {
            game_filter::Enablement::Enabled => "label-enabled",
            game_filter::Enablement::Disabled => "label-disabled",
        })
    }

    pub fn filter_freshness(&self, filter: game_filter::Change) -> String {
        translate(match filter {
            game_filter::Change::New => "label-new",
            game_filter::Change::Updated => "label-updated",
            game_filter::Change::Unscanned => "label-unscanned",
            game_filter::Change::Unchanged => "label-unchanged",
        })
    }

    pub fn backup_format(&self, key: &BackupFormat) -> String {
        translate(match key {
            BackupFormat::Simple => "backup-format-simple",
            BackupFormat::Zip => "backup-format-zip",
        })
    }

    pub fn backup_compression(&self, key: &ZipCompression) -> String {
        translate(match key {
            ZipCompression::None => "compression-none",
            ZipCompression::Deflate => "compression-deflate",
            ZipCompression::Bzip2 => "compression-bzip2",
            ZipCompression::Zstd => "compression-zstd",
        })
    }

    pub fn theme_name(&self, theme: &Theme) -> String {
        translate(match theme {
            Theme::Light => "theme-light",
            Theme::Dark => "theme-dark",
        })
    }


    pub fn game_label(&self) -> String {
        translate("label-game")
    }

    pub fn alias_label(&self) -> String {
        translate("label-alias")
    }

    pub fn original_name_label(&self) -> String {
        translate("label-original-name")
    }

    pub fn original_name_field(&self) -> String {
        self.field(&self.original_name_label())
    }

    pub fn source_label(&self) -> String {
        translate("label-source")
    }

    pub fn source_field(&self) -> String {
        self.field(&self.source_label())
    }

    pub fn primary_manifest_label(&self) -> String {
        translate("label-primary-manifest")
    }

    pub fn integration_label(&self) -> String {
        translate("label-integration")
    }

    pub fn custom_game_kind(&self, kind: &CustomGameKind) -> String {
        match kind {
            CustomGameKind::Game => self.game_label(),
            CustomGameKind::Alias => self.alias_label(),
        }
    }

    pub fn custom_game_name_placeholder(&self) -> String {
        translate("game-name")
    }

    pub fn search_game_name_placeholder(&self) -> String {
        translate("game-name")
    }

    pub fn show_disabled_games(&self) -> String {
        translate("show-disabled-games")
    }

    pub fn show_unchanged_games(&self) -> String {
        translate("show-unchanged-games")
    }

    pub fn show_unscanned_games(&self) -> String {
        translate("show-unscanned-games")
    }

    pub fn override_max_threads(&self) -> String {
        format!(
            "{} ({})",
            translate("override-max-threads"),
            self.suffix_restart_required()
        )
    }

    pub fn explanation_for_exclude_store_screenshots(&self) -> String {
        translate("explanation-for-exclude-store-screenshots")
    }

    pub fn explanation_for_exclude_cloud_games(&self) -> String {
        translate("explanation-for-exclude-cloud-games")
    }

    pub fn roots_label(&self) -> String {
        translate("field-roots")
    }

    pub fn wine_prefix(&self) -> String {
        self.store(&Store::OtherWine)
    }

    pub fn ignored_items_label(&self) -> String {
        translate("field-backup-excluded-items")
    }

    pub fn backup_format_field(&self) -> String {
        translate("field-backup-format")
    }

    pub fn backup_compression_field(&self) -> String {
        translate("field-backup-compression")
    }

    pub fn backup_compression_level_field(&self) -> String {
        translate("field-backup-compression-level")
    }

    pub fn manifest_label(&self) -> String {
        self.field(&translate("label-manifest"))
    }

    pub fn checked_label(&self) -> String {
        self.field(&translate("label-checked"))
    }

    pub fn updated_label(&self) -> String {
        self.field(&translate("label-updated"))
    }

    pub fn comment_label(&self) -> String {
        translate("label-comment")
    }

    pub fn backup_label(&self) -> String {
        translate("label-backup")
    }

    pub fn backup_field(&self) -> String {
        self.field(&self.backup_label())
    }

    pub fn scan_label(&self) -> String {
        translate("label-scan")
    }

    pub fn scan_field(&self) -> String {
        self.field(&self.scan_label())
    }

    pub fn filter_label(&self) -> String {
        self.field(&translate("label-filter"))
    }

    pub fn threads_label(&self) -> String {
        self.field(&translate("label-threads"))
    }

    pub fn rclone_label(&self) -> String {
        self.field("Rclone")
    }

    pub fn remote_label(&self) -> String {
        self.field(&translate("label-remote"))
    }

    pub fn remote_name_label(&self) -> String {
        self.field(&translate("label-remote-name"))
    }

    pub fn folder_label(&self) -> String {
        self.field(&translate("label-folder"))
    }

    pub fn executable_label(&self) -> String {
        translate("label-executable")
    }

    pub fn arguments_label(&self) -> String {
        translate("label-arguments")
    }

    pub fn file_label(&self) -> String {
        translate("label-file")
    }

    pub fn url_label(&self) -> String {
        translate("label-url")
    }

    pub fn url_field(&self) -> String {
        self.field(&translate("label-url"))
    }

    pub fn host_label(&self) -> String {
        self.field(&translate("label-host"))
    }

    pub fn port_label(&self) -> String {
        self.field(&translate("label-port"))
    }

    pub fn username_label(&self) -> String {
        self.field(&translate("label-username"))
    }

    pub fn password_label(&self) -> String {
        self.field(&translate("label-password"))
    }

    pub fn provider_label(&self) -> String {
        self.field(&translate("label-provider"))
    }

    pub fn none_label(&self) -> String {
        translate("label-none")
    }

    pub fn custom_label(&self) -> String {
        translate("label-custom")
    }


    pub fn synchronize_automatically(&self) -> String {
        translate("synchronize-automatically")
    }

    pub fn prefer_alias_display(&self) -> String {
        translate("prefer-alias-display")
    }

    pub fn skip_unconstructive_backups(&self) -> String {
        translate("skip-unconstructive-backups")
    }

    pub fn total_games(&self) -> String {
        translate("total-games")
    }

    pub fn new_tooltip(&self) -> String {
        translate("label-new")
    }

    pub fn updated_tooltip(&self) -> String {
        translate("label-updated")
    }

    pub fn removed_tooltip(&self) -> String {
        translate("label-removed")
    }

    fn consider_doing_a_preview(&self) -> String {
        translate("consider-doing-a-preview")
    }

    pub fn confirm_backup(&self, target: &StrictPath, target_exists: bool, suggest: bool) -> String {
        let action_suffix = if target_exists {
            "New save data will be merged into the target folder:"
        } else {
            "The target folder will be created:"
        };
        let primary = format!("Are you sure you want to proceed with the backup? {action_suffix}");

        if suggest {
            format!(
                "{}\n\n{}\n\n{}",
                primary,
                target.render(),
                self.consider_doing_a_preview(),
            )
        } else {
            format!("{}\n\n{}", primary, target.render())
        }
    }

    pub fn confirm_restore(&self, source: &StrictPath, suggest: bool) -> String {
        let primary = translate("confirm-restore");

        if suggest {
            format!(
                "{}\n\n{}\n\n{}",
                primary,
                source.render(),
                self.consider_doing_a_preview(),
            )
        } else {
            format!("{}\n\n{}", primary, source.render(),)
        }
    }

    pub fn notify_single_game_status(&self, found: bool) -> String {
        if found {
            translate("saves-found")
        } else {
            translate("no-saves-found")
        }
    }

    pub fn suffix_no_confirmation(&self) -> String {
        translate("suffix-no-confirmation")
    }

    pub fn suffix_restart_required(&self) -> String {
        translate("suffix-restart-required")
    }

    pub fn prefix_error(&self, message: &str) -> String {
        let mut args = LangArgs::new();
        args.set(MESSAGE, message);
        translate_args("prefix-error", &args)
    }

    pub fn prefix_warning(&self, message: &str) -> String {
        let mut args = LangArgs::new();
        args.set(MESSAGE, message);
        translate_args("prefix-warning", &args)
    }

    pub fn rclone_unavailable(&self) -> String {
        let mut args = LangArgs::new();
        args.set(APP, "Rclone");
        translate_args("cloud-app-unavailable", &args)
    }

    pub fn game_is_unrecognized(&self) -> String {
        translate("game-is-unrecognized")
    }

    pub fn game_has_nothing_to_restore(&self) -> String {
        translate("game-has-nothing-to-restore")
    }

    pub fn launch_game_after_error(&self) -> String {
        translate("launch-game-after-error")
    }

    pub fn game_did_not_launch(&self) -> String {
        translate("game-did-not-launch")
    }

    pub fn backup_is_newer_than_current_data(&self) -> String {
        translate("backup-is-newer-than-current-data")
    }

    pub fn backup_is_older_than_current_data(&self) -> String {
        translate("backup-is-older-than-current-data")
    }

    pub fn back_up_one_game_confirm(&self, game: &str) -> String {
        let mut args = LangArgs::new();
        args.set(GAME, game);
        translate_args("back-up-specific-game.confirm", &args)
    }

    pub fn back_up_one_game_failed(&self, game: &str) -> String {
        let mut args = LangArgs::new();
        args.set(GAME, game);
        translate_args("back-up-specific-game.failed", &args)
    }

    pub fn restore_one_game_confirm(&self, game: &str) -> String {
        let mut args = LangArgs::new();
        args.set(GAME, game);
        translate_args("restore-specific-game.confirm", &args)
    }

    pub fn restore_one_game_failed(&self, game: &str) -> String {
        let mut args = LangArgs::new();
        args.set(GAME, game);
        translate_args("restore-specific-game.failed", &args)
    }

    pub fn new_version_check(&self) -> String {
        translate("new-version-check")
    }

    pub fn new_version_available(&self, version: &str) -> String {
        let mut args = LangArgs::new();
        args.set(VERSION, version);
        translate_args("new-version-available", &args)
    }

    pub fn custom_game_will_override(&self) -> String {
        translate("custom-game-will-override")
    }

    pub fn custom_game_will_extend(&self) -> String {
        translate("custom-game-will-extend")
    }

    pub fn operation_will_only_include_listed_games(&self) -> String {
        translate("operation-will-only-include-listed-games")
    }
}

#[cfg(test)]
mod tests {
    use crate::lang::TRANSLATOR;
    use pretty_assertions::assert_eq;

    #[test]
    fn adjusted_size() {
        assert_eq!("0 B", &TRANSLATOR.adjusted_size(0));
        assert_eq!("1 B", &TRANSLATOR.adjusted_size(1));
        assert_eq!("1.03 KiB", &TRANSLATOR.adjusted_size(1_050));
        assert_eq!("100.00 KiB", &TRANSLATOR.adjusted_size(102_400));
        assert_eq!("114.98 GiB", &TRANSLATOR.adjusted_size(123_456_789_000));
    }
}
