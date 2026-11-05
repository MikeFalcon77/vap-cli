//! Configuration resolution belongs to the application boundary, never to parameter assembly.
use crate::{
    config::{self, Config, Profile},
    error::{Error, Result},
    parameters::Targets,
    request::Authentication,
};
use std::{path::PathBuf, time::Duration};
use zeroize::Zeroizing;

pub struct Bootstrap {
    pub config: Config,
    pub profile: String,
}
impl Bootstrap {
    pub fn load(args: &[String]) -> Result<Self> {
        let config = Config::load()?;
        let profile = args
            .windows(2)
            .find(|a| a[0] == "--profile")
            .map(|a| a[1].clone())
            .or_else(|| {
                args.iter()
                    .find_map(|a| a.strip_prefix("--profile=").map(String::from))
            })
            .or_else(|| std::env::var("VAP_PROFILE").ok())
            .or_else(|| config.default_profile.clone())
            .unwrap_or("default".into());
        config::validate_profile(&profile)?;
        Ok(Self { config, profile })
    }
}
pub struct ExecutionContext {
    pub profile_name: String,
    pub platform: Option<String>,
    pub targets: Targets,
    pub timeout: Duration,
    pub interval: Duration,
    pub ca: Option<PathBuf>,
    profile: Option<Profile>,
}
pub struct AuthorizedContext {
    pub context: ExecutionContext,
    pub credential: Option<Zeroizing<String>>,
}
impl ExecutionContext {
    pub fn new(
        config: &Config,
        name: &str,
        platform: Option<String>,
        timeout: Duration,
        interval: Duration,
        ca: Option<PathBuf>,
    ) -> Result<Self> {
        let profile = config.profiles.get(name).cloned();
        let platform = platform
            .or_else(|| std::env::var("VAP_PLATFORM").ok())
            .or_else(|| profile.as_ref().map(|p| p.platform.clone()))
            .map(|p| config::platform(&p))
            .transpose()?;
        let targets = Targets {
            environment: profile.as_ref().and_then(|p| p.environment.clone()),
            project: profile.as_ref().and_then(|p| p.project.clone()),
            node: profile.as_ref().and_then(|p| p.node.clone()),
        };
        Ok(Self {
            profile_name: name.into(),
            platform,
            targets,
            timeout,
            interval,
            ca,
            profile,
        })
    }
    // Consumes the unresolved context: credentials can only be resolved once per operation.
    pub async fn authorize(self, mode: Authentication) -> Result<AuthorizedContext> {
        let platform = self.platform.as_deref().ok_or_else(|| {
            Error::new(
                "NOT_AUTHENTICATED",
                "Set --platform / VAP_PLATFORM or configure a profile",
            )
        })?;
        let credential = if mode == Authentication::None {
            None
        } else if std::env::var_os("VAP_TOKEN").is_some() {
            Some(config::validate_token(
                std::env::var("VAP_TOKEN")
                    .map_err(|_| Error::new("INVALID_INPUT", "VAP_TOKEN is not UTF-8"))?,
            )?)
        } else if let Some(path) = std::env::var_os("VAP_TOKEN_FILE") {
            Some(config::validate_token(std::fs::read_to_string(path)?)?)
        } else if let Some(p) = &self.profile {
            if p.platform != platform {
                return Err(Error::new(
                    "PERMISSION_DENIED",
                    "Refusing to send a stored credential to another platform",
                )
                .hint("Create a separate profile or explicitly supply VAP_TOKEN"));
            }
            let p = p.clone();
            Some(
                tokio::task::spawn_blocking(move || config::stored_credential(&p))
                    .await
                    .map_err(|_| Error::new("INTERNAL_ERROR", "Credential operation failed"))??,
            )
        } else if mode == Authentication::Optional {
            None
        } else {
            return Err(Error::new(
                "NOT_AUTHENTICATED",
                "No credential; run vap auth import/login or supply VAP_TOKEN",
            ));
        };
        Ok(AuthorizedContext {
            context: self,
            credential,
        })
    }
}
