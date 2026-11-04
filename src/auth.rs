use crate::{
    config::{self, Config, Profile},
    error::{Error, Result},
};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    io::{self, IsTerminal, Read, Write},
    path::{Path, PathBuf},
};
use zeroize::Zeroizing;

#[derive(Serialize)]
pub struct Source {
    pub source: String,
    pub platform: String,
    pub credential_kind: &'static str,
}

pub fn interactive(json: bool, non_interactive: bool) -> bool {
    !json
        && !non_interactive
        && io::stdin().is_terminal()
        && io::stderr().is_terminal()
        && std::env::var_os("CI").is_none()
}

pub fn confirm(message: &str, yes: bool, interactive: bool) -> Result<()> {
    if yes {
        return Ok(());
    }
    if !interactive {
        return Err(
            Error::new("CONFIRMATION_REQUIRED", "Explicit confirmation is required")
                .hint("Review the target and repeat with --yes"),
        );
    }
    eprint!("{message} [y/N] ");
    io::stderr().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
        return Err(Error::new("CANCELLED", "No changes made"));
    }
    Ok(())
}

pub fn read_secret(
    stdin: bool,
    file: Option<&Path>,
    can_prompt: bool,
    prompt: &str,
) -> Result<Zeroizing<String>> {
    if stdin && file.is_some() {
        return Err(Error::new(
            "INVALID_INPUT",
            "Choose stdin or a credential file, not both",
        ));
    }
    if let Some(file) = file {
        return config::validate_token(std::fs::read_to_string(file)?);
    }
    if stdin {
        let mut text = String::new();
        io::stdin().take(64 * 1024).read_to_string(&mut text)?;
        return config::validate_token(text);
    }
    if !can_prompt {
        return Err(Error::new("NOT_AUTHENTICATED","Credential input is required").hint("Supply VAP_TOKEN, --token-stdin or --token-file; secrets are never command arguments"));
    }
    config::validate_token(rpassword::prompt_password(prompt)?)
}

pub fn read_password(stdin: bool, can_prompt: bool) -> Result<Zeroizing<String>> {
    let mut value = if stdin {
        let mut text = String::new();
        io::stdin().take(64 * 1024).read_to_string(&mut text)?;
        if text.ends_with('\n') {
            text.pop();
            if text.ends_with('\r') {
                text.pop();
            }
        }
        text
    } else if can_prompt {
        rpassword::prompt_password("Password: ")?
    } else {
        return Err(Error::new(
            "NOT_AUTHENTICATED",
            "Password input requires a terminal or --password-stdin",
        ));
    };
    if value.is_empty() {
        use zeroize::Zeroize;
        value.zeroize();
        return Err(Error::new("INVALID_INPUT", "Password cannot be empty"));
    }
    Ok(Zeroizing::new(value))
}

pub fn discover() -> Result<Vec<Source>> {
    let mut candidates = vec![];
    if let Some(home) = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }) {
        let path = PathBuf::from(home).join(".config/jelastic/jelastic.properties");
        if path.is_file() {
            candidates.push(path.to_string_lossy().into_owned());
        }
    }
    #[cfg(windows)]
    {
        if let Ok(output) = std::process::Command::new("wsl.exe")
            .args(["--list", "--quiet"])
            .output()
        {
            let text = if output.stdout.contains(&0) {
                let units: Vec<_> = output
                    .stdout
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .copied()
                    .map(u16::from_le_bytes)
                    .collect();
                String::from_utf16_lossy(&units)
            } else {
                String::from_utf8_lossy(&output.stdout).into_owned()
            };
            for distro in text
                .lines()
                .map(|s| s.trim().trim_start_matches('\u{feff}'))
                .filter(|s| !s.is_empty())
            {
                candidates.push(format!("wsl:{distro}"));
            }
        }
    }
    let mut sources = vec![];
    for source in candidates {
        if let Ok((platform, _)) = read_legacy(&source) {
            sources.push(Source {
                source,
                platform,
                credential_kind: "session",
            });
        }
    }
    Ok(sources)
}

pub fn read_legacy(source: &str) -> Result<(String, Zeroizing<String>)> {
    let text = if let Some(distro) = source.strip_prefix("wsl:") {
        #[cfg(windows)]
        {
            if distro.is_empty()
                || !distro
                    .chars()
                    .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | ' ' | '.'))
            {
                return Err(Error::new("INVALID_INPUT", "Invalid WSL distribution name"));
            }
            let home = std::process::Command::new("wsl.exe")
                .args(["-d", distro, "--", "printenv", "HOME"])
                .output()?;
            if !home.status.success() {
                return Err(Error::new("IO_ERROR", "Cannot locate WSL home directory"));
            }
            let home = String::from_utf8_lossy(&home.stdout).trim().to_string();
            let mut output = std::process::Command::new("wsl.exe")
                .args([
                    "-d",
                    distro,
                    "--",
                    "cat",
                    &format!("{home}/.config/jelastic/jelastic.properties"),
                ])
                .output()?;
            if !output.status.success() {
                return Err(Error::new(
                    "IO_ERROR",
                    "Cannot read legacy WSL configuration",
                ));
            }
            let text = Zeroizing::new(String::from_utf8_lossy(&output.stdout).into_owned());
            use zeroize::Zeroize;
            output.stdout.zeroize();
            text
        }
        #[cfg(not(windows))]
        {
            let _ = distro;
            return Err(Error::new(
                "INVALID_INPUT",
                "WSL import is available from native Windows builds",
            ));
        }
    } else {
        Zeroizing::new(std::fs::read_to_string(source)?)
    };
    let mut props = properties(&text)?;
    let platform =
        config::platform(props.get("jelastic.cli.platform_url").ok_or_else(|| {
            Error::new("INVALID_INPUT", "Legacy configuration has no platform URL")
        })?)?;
    let token = props
        .remove("jelastic.cli.session")
        .ok_or_else(|| Error::new("NOT_AUTHENTICATED", "Legacy configuration has no session"))?;
    use zeroize::Zeroize;
    for value in props.values_mut() {
        value.zeroize();
    }
    Ok((platform, config::validate_token(token)?))
}

pub fn save_profile(
    name: &str,
    platform: String,
    token: &str,
    store: &str,
    kind: &str,
    source: Option<String>,
) -> Result<serde_json::Value> {
    config::validate_profile(name)?;
    let mut conf = Config::load()?;
    let old = conf.profiles.get(name).cloned();
    let id = format!(
        "vap-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    let profile = Profile {
        platform: platform.clone(),
        credential_store: store.into(),
        credential_kind: kind.into(),
        credential_id: id,
        imported_from: source,
        environment: old.as_ref().and_then(|p| p.environment.clone()),
        project: old.as_ref().and_then(|p| p.project.clone()),
        node: old.as_ref().and_then(|p| p.node.clone()),
    };
    config::store(&profile, token)?;
    conf.profiles.insert(name.into(), profile.clone());
    if conf.default_profile.is_none() {
        conf.default_profile = Some(name.into());
    }
    if let Err(e) = conf.save() {
        let _ = config::forget(&profile);
        return Err(e);
    }
    if let Some(old) = old
        && config::forget(&old).is_err()
    {
        eprintln!(
            "Previous credential could not be removed from its store; remove it manually from vap-cli entries"
        );
    }
    Ok(
        serde_json::json!({"profile":name,"platform":platform,"credential_store":store,"credential_kind":kind,"credential":"[REDACTED]"}),
    )
}

// Java Properties escaping, including continuations and UTF-16 surrogate pairs.
pub fn properties(text: &str) -> Result<BTreeMap<String, String>> {
    fn unescape(s: &str) -> Result<String> {
        let mut units = vec![];
        let mut chars = s.chars();
        while let Some(c) = chars.next() {
            let c = if c == '\\' {
                match chars.next() {
                    Some('u') => {
                        let digits: String = chars.by_ref().take(4).collect();
                        let n = u16::from_str_radix(&digits, 16).map_err(|_| {
                            Error::new("INVALID_INPUT", "Invalid Java Properties Unicode escape")
                        })?;
                        if digits.len() != 4 {
                            return Err(Error::new(
                                "INVALID_INPUT",
                                "Invalid Java Properties Unicode escape",
                            ));
                        }
                        units.push(n);
                        continue;
                    }
                    Some('n') => '\n',
                    Some('r') => '\r',
                    Some('t') => '\t',
                    Some('f') => '\u{000c}',
                    Some(c) => c,
                    None => '\\',
                }
            } else {
                c
            };
            let mut buf = [0; 2];
            units.extend_from_slice(c.encode_utf16(&mut buf));
        }
        String::from_utf16(&units)
            .map_err(|_| Error::new("INVALID_INPUT", "Invalid Java Properties Unicode"))
    }
    let mut logical = vec![];
    let mut current = String::new();
    for line in text.lines() {
        let line = if current.is_empty() {
            line
        } else {
            line.trim_start()
        };
        current.push_str(line);
        if current.chars().rev().take_while(|c| *c == '\\').count() % 2 == 1 {
            current.pop();
            continue;
        }
        logical.push(std::mem::take(&mut current));
    }
    if !current.is_empty() {
        logical.push(current);
    }
    let mut out = BTreeMap::new();
    for line in logical {
        let line = line.trim_start();
        if line.is_empty() || line.starts_with(['#', '!']) {
            continue;
        }
        let mut escaped = false;
        let mut split = None;
        for (i, c) in line.char_indices() {
            if !escaped && (c == '=' || c == ':' || c.is_ascii_whitespace()) {
                split = Some(i);
                break;
            }
            if c == '\\' {
                escaped = !escaped;
            } else {
                escaped = false;
            }
        }
        let (key, tail) = split
            .map(|i| (&line[..i], &line[i..]))
            .unwrap_or((line, ""));
        let tail = tail.trim_start();
        let value = tail.strip_prefix(['=', ':']).unwrap_or(tail).trim_start();
        out.insert(unescape(key)?, unescape(value)?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn java_properties_escapes() {
        let p=properties("# comment\njelastic.cli.platform_url=https\\://example.com\nkey\\=name : a\\=b\\\\c\nemoji=\\uD83D\\uDE80\ncontinued=hello\\\n world\n").unwrap();
        assert_eq!(p["jelastic.cli.platform_url"], "https://example.com");
        assert_eq!(p["key=name"], "a=b\\c");
        assert_eq!(p["emoji"], "🚀");
        assert_eq!(p["continued"], "helloworld");
    }
}
