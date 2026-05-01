use crate::{
    lang::TRANSLATOR,
    prelude::{run_command, CommandError, CommandOutput, Error, Privacy},
    resource::config::{App, Config},
};

pub fn validate_cloud_config(config: &Config, cloud_path: &str) -> Result<Remote, Error> {
    if !config.apps.rclone.is_valid() {
        return Err(Error::RcloneUnavailable);
    }
    let Some(remote) = config.cloud.remote.clone() else {
        return Err(Error::CloudNotConfigured);
    };
    validate_cloud_path(cloud_path)?;
    Ok(remote)
}

pub fn validate_cloud_path(path: &str) -> Result<(), Error> {
    if path.is_empty() || path == "/" {
        Err(Error::CloudPathInvalid)
    } else {
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteChoice {
    None,
    Custom,
    Box,
    Dropbox,
    Ftp,
    GoogleDrive,
    OneDrive,
    Smb,
    WebDav,
}

impl RemoteChoice {
    pub const ALL: &'static [Self] = &[
        Self::None,
        Self::Box,
        Self::Dropbox,
        Self::GoogleDrive,
        Self::OneDrive,
        Self::Ftp,
        Self::Smb,
        Self::WebDav,
        Self::Custom,
    ];
}

impl ToString for RemoteChoice {
    fn to_string(&self) -> String {
        match self {
            Self::None => TRANSLATOR.none_label(),
            Self::Custom => TRANSLATOR.custom_label(),
            Self::Box => "Box".to_string(),
            Self::Dropbox => "Dropbox".to_string(),
            Self::Ftp => "FTP".to_string(),
            Self::GoogleDrive => "Google Drive".to_string(),
            Self::OneDrive => "OneDrive".to_string(),
            Self::Smb => "SMB".to_string(),
            Self::WebDav => "WebDAV".to_string(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename = "camelCase")] // Legacy: Should have been `rename_all`
pub enum Remote {
    Custom {
        id: String,
    },
    Box {
        id: String,
    },
    Dropbox {
        id: String,
    },
    GoogleDrive {
        id: String,
    },
    OneDrive {
        id: String,
    },
    Ftp {
        id: String,
        host: String,
        port: i32,
        username: String,
        #[serde(skip, default)]
        password: String,
    },
    Smb {
        id: String,
        host: String,
        port: i32,
        username: String,
        #[serde(skip, default)]
        password: String,
    },
    WebDav {
        id: String,
        url: String,
        username: String,
        #[serde(skip, default)]
        password: String,
        provider: WebDavProvider,
    },
}

impl Remote {
    pub fn id(&self) -> &str {
        match self {
            Remote::Box { id } => id,
            Remote::Custom { id } => id,
            Remote::Dropbox { id } => id,
            Remote::GoogleDrive { id } => id,
            Remote::OneDrive { id } => id,
            Remote::Ftp { id, .. } => id,
            Remote::Smb { id, .. } => id,
            Remote::WebDav { id, .. } => id,
        }
    }

    pub fn slug(&self) -> &str {
        match self {
            Self::Custom { .. } => "",
            Self::Box { .. } => "box",
            Self::Dropbox { .. } => "dropbox",
            Self::Ftp { .. } => "ftp",
            Self::GoogleDrive { .. } => "drive",
            Self::OneDrive { .. } => "onedrive",
            Self::Smb { .. } => "smb",
            Self::WebDav { .. } => "webdav",
        }
    }

    pub fn config_args(&self) -> Option<Vec<String>> {
        match self {
            Self::Custom { .. } => None,
            Self::Box { .. } => None,
            Self::Dropbox { .. } => None,
            Self::GoogleDrive { .. } => Some(vec!["scope=drive".to_string()]),
            Self::Ftp {
                id: _,
                host,
                port,
                username,
                password,
            } => Some(vec![
                format!("host={host}"),
                format!("port={port}"),
                format!("user={username}"),
                format!("pass={password}"),
            ]),
            Self::OneDrive { .. } => Some(vec![
                "drive_type=personal".to_string(),
                "access_scopes=Files.ReadWrite,offline_access".to_string(),
            ]),
            Self::Smb {
                id: _,
                host,
                port,
                username,
                password,
                ..
            } => Some(vec![
                format!("host={host}"),
                format!("port={port}"),
                format!("user={username}"),
                format!("pass={password}"),
            ]),
            Self::WebDav {
                id: _,
                url,
                username,
                password,
                provider,
            } => Some(vec![
                format!("url={url}"),
                format!("user={username}"),
                format!("pass={password}"),
                format!("vendor={}", provider.slug()),
            ]),
        }
    }

    pub fn needs_configuration(&self) -> bool {
        match self {
            Self::Custom { .. } => false,
            Self::Box { .. }
            | Self::Dropbox { .. }
            | Self::Ftp { .. }
            | Self::GoogleDrive { .. }
            | Self::OneDrive { .. }
            | Self::Smb { .. }
            | Self::WebDav { .. } => true,
        }
    }

    pub fn description(&self) -> Option<String> {
        match self {
            Remote::Ftp {
                host, port, username, ..
            } => Some(format!("{username}@{host}:{port}")),
            Remote::Smb {
                host, port, username, ..
            } => Some(format!("{username}@{host}:{port}")),
            Remote::WebDav { url, provider, .. } => Some(format!("{} - {}", provider.to_string(), url)),
            _ => None,
        }
    }

    pub fn generate_id() -> String {
        format!("ludusavi-{}", chrono::Utc::now().timestamp())
    }
}

impl From<Option<&Remote>> for RemoteChoice {
    fn from(value: Option<&Remote>) -> Self {
        if let Some(value) = value {
            match value {
                Remote::Custom { .. } => RemoteChoice::Custom,
                Remote::Box { .. } => RemoteChoice::Box,
                Remote::Dropbox { .. } => RemoteChoice::Dropbox,
                Remote::Ftp { .. } => RemoteChoice::Ftp,
                Remote::GoogleDrive { .. } => RemoteChoice::GoogleDrive,
                Remote::OneDrive { .. } => RemoteChoice::OneDrive,
                Remote::Smb { .. } => RemoteChoice::Smb,
                Remote::WebDav { .. } => RemoteChoice::WebDav,
            }
        } else {
            RemoteChoice::None
        }
    }
}

impl TryFrom<RemoteChoice> for Remote {
    type Error = ();

    fn try_from(value: RemoteChoice) -> Result<Self, Self::Error> {
        match value {
            RemoteChoice::None => Err(()),
            RemoteChoice::Custom => Ok(Remote::Custom {
                id: "ludusavi".to_string(),
            }),
            RemoteChoice::Box => Ok(Remote::Box {
                id: Remote::generate_id(),
            }),
            RemoteChoice::Dropbox => Ok(Remote::Dropbox {
                id: Remote::generate_id(),
            }),
            RemoteChoice::Ftp => Ok(Remote::Ftp {
                id: Remote::generate_id(),
                host: String::new(),
                port: 21,
                username: String::new(),
                password: String::new(),
            }),
            RemoteChoice::GoogleDrive => Ok(Remote::GoogleDrive {
                id: Remote::generate_id(),
            }),
            RemoteChoice::OneDrive => Ok(Remote::OneDrive {
                id: Remote::generate_id(),
            }),
            RemoteChoice::Smb => Ok(Remote::Smb {
                id: Remote::generate_id(),
                host: String::new(),
                port: 445,
                username: String::new(),
                password: String::new(),
            }),
            RemoteChoice::WebDav => Ok(Remote::WebDav {
                id: Remote::generate_id(),
                url: String::new(),
                username: String::new(),
                password: String::new(),
                provider: WebDavProvider::Other,
            }),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub enum WebDavProvider {
    #[default]
    Other,
    Nextcloud,
    Owncloud,
    Sharepoint,
    SharepointNtlm,
}

impl WebDavProvider {
    pub const ALL: &'static [Self] = &[
        Self::Other,
        Self::Nextcloud,
        Self::Owncloud,
        Self::Sharepoint,
        Self::SharepointNtlm,
    ];

    pub const ALL_CLI: &'static [&'static str] = &[
        Self::OTHER,
        Self::NEXTCLOUD,
        Self::OWNCLOUD,
        Self::SHAREPOINT,
        Self::SHAREPOINT_NTLM,
    ];
    pub const OTHER: &'static str = "other";
    const NEXTCLOUD: &'static str = "nextcloud";
    const OWNCLOUD: &'static str = "owncloud";
    const SHAREPOINT: &'static str = "sharepoint";
    const SHAREPOINT_NTLM: &'static str = "sharepoint-ntlm";
}

impl WebDavProvider {
    pub fn slug(&self) -> &str {
        match self {
            WebDavProvider::Other => Self::OTHER,
            WebDavProvider::Nextcloud => Self::NEXTCLOUD,
            WebDavProvider::Owncloud => Self::OWNCLOUD,
            WebDavProvider::Sharepoint => Self::SHAREPOINT,
            WebDavProvider::SharepointNtlm => Self::SHAREPOINT_NTLM,
        }
    }
}

impl ToString for WebDavProvider {
    fn to_string(&self) -> String {
        match self {
            Self::Other => crate::resource::manifest::Store::Other.to_string(),
            Self::Nextcloud => "Nextcloud".to_string(),
            Self::Owncloud => "Owncloud".to_string(),
            Self::Sharepoint => "Sharepoint".to_string(),
            Self::SharepointNtlm => "Sharepoint (NTLM)".to_string(),
        }
    }
}

impl std::str::FromStr for WebDavProvider {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            Self::OTHER => Ok(Self::Other),
            Self::NEXTCLOUD => Ok(Self::Nextcloud),
            Self::OWNCLOUD => Ok(Self::Owncloud),
            Self::SHAREPOINT => Ok(Self::Sharepoint),
            Self::SHAREPOINT_NTLM => Ok(Self::SharepointNtlm),
            _ => Err(format!("invalid provider: {s}")),
        }
    }
}

pub struct Rclone {
    app: App,
    remote: Remote,
}

impl Rclone {
    pub fn new(app: App, remote: Remote) -> Self {
        Self { app, remote }
    }

    fn args(&self, args: &[String]) -> Vec<String> {
        let mut collected = vec![];
        if !self.app.arguments.is_empty() {
            if let Some(parts) = shlex::split(&self.app.arguments) {
                collected.extend(parts);
            }
        }
        for arg in args {
            collected.push(arg.to_string());
        }
        collected
    }

    fn run(&self, args: &[String], success: &[i32], privacy: Privacy) -> Result<CommandOutput, CommandError> {
        let args = self.args(args);
        let args: Vec<_> = args.iter().map(|x| x.as_str()).collect();
        run_command(self.app.path.raw(), &args, success, privacy)
    }

    fn obscure(&self, credential: &str) -> Result<String, CommandError> {
        let out = self.run(&["obscure".to_string(), credential.to_string()], &[0], Privacy::Private)?;
        Ok(out.stdout)
    }

    pub fn configure_remote(&self) -> Result<(), CommandError> {
        if !self.remote.needs_configuration() {
            return Ok(());
        }

        let mut privacy = Privacy::Public;

        let mut remote = self.remote.clone();
        match &mut remote {
            Remote::Custom { .. }
            | Remote::Box { .. }
            | Remote::Dropbox { .. }
            | Remote::GoogleDrive { .. }
            | Remote::OneDrive { .. } => {}
            Remote::Ftp { password, .. } => {
                privacy = Privacy::Private;
                *password = self.obscure(password)?;
            }
            Remote::Smb { password, .. } => {
                privacy = Privacy::Private;
                *password = self.obscure(password)?;
            }
            Remote::WebDav { password, .. } => {
                privacy = Privacy::Private;
                *password = self.obscure(password)?;
            }
        }

        let mut args = vec![
            "config".to_string(),
            "create".to_string(),
            remote.id().to_string(),
            remote.slug().to_string(),
        ];

        if let Some(config_args) = remote.config_args() {
            args.extend(config_args);
        }

        self.run(&args, &[0], privacy)?;
        Ok(())
    }

    pub fn unconfigure_remote(&self) -> Result<(), CommandError> {
        if !self.remote.needs_configuration() {
            return Ok(());
        }

        let args = vec!["config".to_string(), "delete".to_string(), self.remote.id().to_string()];

        self.run(&args, &[0], Privacy::Public)?;
        Ok(())
    }
}

