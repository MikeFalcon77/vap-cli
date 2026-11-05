use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
};
use zeroize::Zeroizing;

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Config {
    pub default_profile: Option<String>,
    #[serde(default)]
    pub profiles: BTreeMap<String, Profile>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Profile {
    pub platform: String,
    pub credential_store: String,
    pub credential_kind: String,
    pub credential_id: String,
    pub imported_from: Option<String>,
    pub environment: Option<String>,
    pub project: Option<String>,
    pub node: Option<String>,
}

pub fn home() -> Result<PathBuf> {
    std::env::var_os("VAP_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
                .map(|p| PathBuf::from(p).join(".vap-cli"))
        })
        .ok_or_else(|| {
            Error::new(
                "IO_ERROR",
                "Cannot locate home directory; set VAP_CONFIG_DIR",
            )
        })
}

fn path() -> Result<PathBuf> {
    Ok(home()?.join("config.json"))
}

impl Config {
    pub fn load() -> Result<Self> {
        match fs::read_to_string(path()?) {
            Ok(s) => Ok(serde_json::from_str(&s)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }
    pub fn selected<'a>(&'a self, name: Option<&str>) -> Result<(&'a str, &'a Profile)> {
        let name = name
            .or(self.default_profile.as_deref())
            .unwrap_or("default");
        self.profiles
            .get_key_value(name)
            .map(|(k, v)| (k.as_str(), v))
            .ok_or_else(|| {
                Error::new("NOT_AUTHENTICATED", "No configured profile").hint(
                    "Run vap auth login --platform https://app.example.com, or vap auth import",
                )
            })
    }
    pub fn save(&self) -> Result<()> {
        atomic_private_write(&path()?, &serde_json::to_vec_pretty(self)?, true)
    }
}

pub fn validate_profile(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 80
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(Error::new(
            "INVALID_INPUT",
            "Profile names use 1–80 letters, digits, hyphens or underscores",
        ));
    }
    Ok(())
}

pub fn platform(input: &str) -> Result<String> {
    let input = if input.contains("://") {
        input.to_string()
    } else {
        format!("https://{input}")
    };
    let url =
        url::Url::parse(&input).map_err(|_| Error::new("INVALID_INPUT", "Invalid platform URL"))?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err(Error::new(
            "INVALID_INPUT",
            "Platform must be an HTTPS origin without credentials, path or query",
        ));
    }
    Ok(url.as_str().trim_end_matches('/').into())
}

pub fn secure_directory(path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
    if fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(Error::new(
            "IO_ERROR",
            "Configuration directory must not be a symlink",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(windows)]
    protect_acl(path)?;
    Ok(())
}

// The temporary file is private from creation. Atomic replacement also avoids partially written credentials.
pub fn atomic_private_write(path: &Path, bytes: &[u8], replace: bool) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| Error::new("IO_ERROR", "Output path has no parent"))?;
    let parent = if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    };
    // Do not change permissions of arbitrary output directories; only our own config directories.
    if path.starts_with(home()?) {
        secure_directory(&home()?)?;
        secure_directory(parent)?;
    } else if !parent.is_dir() {
        return Err(Error::new("IO_ERROR", "Output directory does not exist"));
    }
    if path.exists() && (!replace || fs::symlink_metadata(path)?.file_type().is_symlink()) {
        return Err(Error::new(
            "IO_ERROR",
            "Output already exists; refusing to overwrite it",
        ));
    }
    let tmp = parent.join(format!(
        ".vap-{}-{}.tmp",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| {
        let mut file = options.open(&tmp)?;
        #[cfg(windows)]
        protect_acl(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        if replace {
            #[cfg(windows)]
            {
                use std::os::windows::ffi::OsStrExt;
                let from: Vec<_> = tmp.as_os_str().encode_wide().chain(Some(0)).collect();
                let to: Vec<_> = path.as_os_str().encode_wide().chain(Some(0)).collect();
                // MoveFileEx replaces atomically without exposing an unprotected new file.
                if unsafe {
                    windows_sys::Win32::Storage::FileSystem::MoveFileExW(
                        from.as_ptr(),
                        to.as_ptr(),
                        windows_sys::Win32::Storage::FileSystem::MOVEFILE_REPLACE_EXISTING
                            | windows_sys::Win32::Storage::FileSystem::MOVEFILE_WRITE_THROUGH,
                    )
                } == 0
                {
                    return Err(Error::from(std::io::Error::last_os_error()));
                }
            }
            #[cfg(not(windows))]
            fs::rename(&tmp, path)?;
        } else {
            // Hard-link creation is atomic and fails rather than overwriting a concurrent writer.
            fs::hard_link(&tmp, path)?;
            fs::remove_file(&tmp)?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

#[cfg(windows)]
fn protect_acl(path: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::{
            Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW,
            DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, SetFileSecurityW,
        },
    };
    let sddl: Vec<u16> = "D:P(A;OICI;FA;;;OW)"
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let mut sd = std::ptr::null_mut();
    let path: Vec<_> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe {
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            1,
            &mut sd,
            std::ptr::null_mut(),
        ) == 0
        {
            return Err(Error::from(std::io::Error::last_os_error()));
        }
        let ok = SetFileSecurityW(
            path.as_ptr(),
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            sd,
        );
        LocalFree(sd);
        if ok == 0 {
            return Err(Error::from(std::io::Error::last_os_error()));
        }
    }
    Ok(())
}

fn entry(profile: &Profile) -> Result<keyring::Entry> {
    keyring::Entry::new("vap-cli", &profile.credential_id).map_err(|_| {
        Error::new("KEYCHAIN_UNAVAILABLE", "Cannot access the system keychain")
            .hint("For headless use explicitly select --store file, or supply VAP_TOKEN")
    })
}

pub fn store(profile: &Profile, token: &str) -> Result<()> {
    validate_profile(&profile.credential_id)?;
    match profile.credential_store.as_str() {
        "keychain" => entry(profile)?.set_password(token).map_err(|_| {
            Error::new(
                "KEYCHAIN_UNAVAILABLE",
                "Could not store credential in the system keychain; no file fallback was used",
            )
            .hint("Explicitly select --store file for headless storage")
        }),
        "file" => atomic_private_write(
            &home()?
                .join("credentials")
                .join(format!("{}.secret", profile.credential_id)),
            token.as_bytes(),
            true,
        ),
        _ => Err(Error::new("INVALID_INPUT", "Unknown credential store")),
    }
}

pub fn credential(profile: &Profile) -> Result<Zeroizing<String>> {
    if let Ok(token) = std::env::var("VAP_TOKEN") {
        return validate_token(token);
    }
    if let Some(path) = std::env::var_os("VAP_TOKEN_FILE") {
        return validate_token(fs::read_to_string(path)?);
    }
    stored_credential(profile)
}
pub fn stored_credential(profile: &Profile) -> Result<Zeroizing<String>> {
    let token = match profile.credential_store.as_str() {
        "keychain" => entry(profile)?.get_password().map_err(|_| {
            Error::new(
                "NOT_AUTHENTICATED",
                "Credential is absent or keychain is unavailable",
            )
            .hint("Run vap auth login/import, or supply VAP_TOKEN")
        })?,
        "file" => {
            validate_profile(&profile.credential_id)?;
            let path = home()?
                .join("credentials")
                .join(format!("{}.secret", profile.credential_id));
            if fs::symlink_metadata(&path)?.file_type().is_symlink() {
                return Err(Error::new(
                    "PERMISSION_DENIED",
                    "Credential file must not be a symlink",
                ));
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if fs::metadata(&path)?.permissions().mode() & 0o077 != 0 {
                    return Err(Error::new(
                        "PERMISSION_DENIED",
                        "Credential file permissions must be 0600",
                    ));
                }
            }
            fs::read_to_string(path)?
        }
        _ => return Err(Error::new("INVALID_INPUT", "Unknown credential store")),
    };
    validate_token(token)
}

pub fn validate_token(token: String) -> Result<Zeroizing<String>> {
    let token = Zeroizing::new(token);
    let trimmed = token.trim();
    if trimmed.is_empty() || trimmed.chars().any(char::is_whitespace) {
        return Err(Error::new(
            "INVALID_INPUT",
            "Credential must be a nonempty value without whitespace",
        ));
    }
    Ok(Zeroizing::new(trimmed.into()))
}

pub fn forget(profile: &Profile) -> Result<()> {
    validate_profile(&profile.credential_id)?;
    match profile.credential_store.as_str() {
        "keychain" => match entry(profile)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err(Error::new(
                "KEYCHAIN_UNAVAILABLE",
                "Cannot remove credential from keychain",
            )),
        },
        "file" => match fs::remove_file(
            home()?
                .join("credentials")
                .join(format!("{}.secret", profile.credential_id)),
        ) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        },
        _ => Err(Error::new("INVALID_INPUT", "Unknown credential store")),
    }
}
