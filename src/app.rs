use crate::{
    auth,
    catalog::{self, Catalog, Method},
    cli, config,
    error::{Error, Result},
    output, transport,
};
use clap::ArgMatches;
use serde_json::{Map, Value, json};
use std::{
    io::{self, Read},
    path::Path,
    time::Duration,
};

pub async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let json_mode = args.iter().any(|s| s == "--json");
    let result=async {
        let catalog=load_catalog(&args)?;
        let mut command=cli::build(&catalog);
        let matches=match command.clone().try_get_matches_from(&args) {
            Ok(m)=>m,
            Err(e) if matches!(e.kind(),clap::error::ErrorKind::DisplayHelp|clap::error::ErrorKind::DisplayVersion) || (!json_mode && e.kind()==clap::error::ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand)=>{e.print().map_err(Error::from)?;return Ok(());},
            Err(_)=>return Err(Error::new("INVALID_INPUT","Invalid command or option").hint("Use vap --help or the command's --help; secrets must use file/stdin")),
        };
        tokio::select! {
            result=run(&matches,&catalog,&mut command)=>result,
            _=tokio::signal::ctrl_c()=>Err(Error::new("INTERRUPTED","Stopped waiting; any server operation may continue").hint("Inspect operations and resource state before retrying")),
        }
    }.await;
    if let Err(error) = result {
        output::failure(&error, json_mode);
        std::process::exit(error.exit_code());
    }
}

fn str_arg<'a>(m: &'a ArgMatches, name: &str) -> Option<&'a str> {
    m.try_get_one::<String>(name)
        .ok()
        .flatten()
        .map(String::as_str)
}
fn flag(m: &ArgMatches, name: &str) -> bool {
    m.try_get_one::<bool>(name)
        .ok()
        .flatten()
        .copied()
        .unwrap_or(false)
}
fn num(m: &ArgMatches, name: &str) -> u64 {
    m.get_one::<u64>(name).copied().unwrap_or(600)
}
fn can_prompt(m: &ArgMatches) -> bool {
    auth::interactive(flag(m, "json"), flag(m, "non-interactive"))
}
fn profile_name(m: &ArgMatches) -> String {
    str_arg(m, "profile")
        .map(String::from)
        .or_else(|| std::env::var("VAP_PROFILE").ok())
        .unwrap_or_else(|| {
            config::Config::load()
                .ok()
                .and_then(|c| c.default_profile)
                .unwrap_or("default".into())
        })
}
fn load_catalog(args: &[String]) -> Result<Catalog> {
    let name = args
        .windows(2)
        .find(|a| a[0] == "--profile")
        .map(|a| a[1].clone())
        .or_else(|| {
            args.iter()
                .find_map(|a| a.strip_prefix("--profile=").map(String::from))
        })
        .or_else(|| std::env::var("VAP_PROFILE").ok())
        .or_else(|| config::Config::load().ok().and_then(|c| c.default_profile))
        .unwrap_or("default".into());
    config::validate_profile(&name)?;
    let path = config::home()?
        .join("catalogs")
        .join(format!("{name}.json"));
    if path.exists() {
        Catalog::parse(&read_text(&path)?)
    } else {
        Catalog::embedded()
    }
}
fn read_text(path: &Path) -> Result<String> {
    let mut text = String::new();
    if path == Path::new("-") {
        io::stdin()
            .take(16 * 1024 * 1024 + 1)
            .read_to_string(&mut text)?;
    } else {
        std::fs::File::open(path)?
            .take(16 * 1024 * 1024 + 1)
            .read_to_string(&mut text)?;
    }
    if text.len() > 16 * 1024 * 1024 {
        return Err(Error::new("INVALID_INPUT", "Input exceeds 16 MiB"));
    }
    Ok(text)
}
fn emit(data: Value, m: &ArgMatches, mut meta: Value) -> Result<()> {
    let fields: Vec<String> = m
        .get_many::<String>("fields")
        .map(|v| v.cloned().collect())
        .unwrap_or_default();
    let (data, projection) = output::project(data, &fields, m.get_one::<usize>("limit").copied());
    if let (Some(meta), Some(projection)) = (meta.as_object_mut(), projection.as_object()) {
        meta.extend(projection.clone());
    }
    output::success(data, meta, flag(m, "json"))
}
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|_| Error::new("INTERNAL_ERROR", "Credential operation failed"))?
}

async fn run(m: &ArgMatches, catalog: &Catalog, command: &mut clap::Command) -> Result<()> {
    match m.subcommand() {
        Some(("completion", sub)) => {
            let shell = str_arg(sub, "shell")
                .unwrap()
                .parse::<clap_complete::Shell>()
                .map_err(|_| Error::new("INVALID_INPUT", "Unsupported shell"))?;
            clap_complete::generate(shell, command, "vap", &mut io::stdout());
            Ok(())
        }
        Some(("doctor", _)) => {
            let conf = config::Config::load()?;
            emit(
                json!({"version":env!("CARGO_PKG_VERSION"),"os":std::env::consts::OS,"arch":std::env::consts::ARCH,"config_directory":config::home()?,"profiles":conf.profiles.keys().collect::<Vec<_>>(),"catalog":catalog.stats(),"http_backend":crate::http_client::NAME,"tls":crate::http_client::TLS,"telemetry":false,"automatic_updates":false}),
                m,
                json!({}),
            )
        }
        Some(("profile", sub)) => run_profile(m, sub),
        Some(("auth", sub)) => run_auth(m, sub).await,
        Some(("api", sub)) => match sub.subcommand() {
            Some(("search", s)) => {
                let query = str_arg(s, "query").unwrap().to_ascii_lowercase();
                let found:Vec<_>=catalog.methods.iter().filter(|method|method.path.contains(&query)&&(!flag(s,"public-only")||method.public)).map(|method|json!({"method":format!("{}.{}.{}",method.namespace,method.service,method.name),"path":method.path,"public":method.public,"read_only":method.read_only()})).collect();
                emit(json!(found), m, json!({"catalog":catalog.stats()}))
            }
            Some(("describe", s)) => {
                let method = catalog.find(str_arg(s, "method").unwrap()).ok_or_else(|| {
                    Error::new("UNKNOWN_METHOD", "Method is absent from this catalog")
                        .hint("Use vap api search or vap api call <namespace/service/rest/method>")
                })?;
                emit(
                    json!({"method":method,"read_only":method.read_only(),"availability":"Not live-verified; depends on platform version and permissions","sensitive_input":"Use --input file.json or --input -; session is injected","platform_version":catalog.platform_version}),
                    m,
                    json!({}),
                )
            }
            Some(("schema", s)) => {
                if let Some(("refresh", s)) = s.subcommand() {
                    refresh(m, s, catalog)
                } else {
                    emit(
                        serde_json::to_value(catalog)?,
                        m,
                        json!({"catalog":catalog.stats()}),
                    )
                }
            }
            Some(("call", s)) => {
                let path = str_arg(s, "path")
                    .unwrap()
                    .trim_matches('/')
                    .to_ascii_lowercase();
                catalog::validate_path(&path)?;
                let method = catalog.find(&path).cloned().unwrap_or_else(|| {
                    let parts: Vec<_> = path.split('/').collect();
                    Method {
                        path: path.clone(),
                        namespace: parts[0].into(),
                        service: parts[1].into(),
                        name: parts[3].into(),
                        public: false,
                        doc_url: None,
                        parameters: vec![],
                    }
                });
                execute(m, s, &method, false, false).await
            }
            Some((namespace, s)) => {
                let (service, s) = s.subcommand().unwrap();
                let (name, s) = s.subcommand().unwrap();
                let path = format!("{namespace}/{service}/rest/{name}");
                let method = catalog
                    .find(&path)
                    .ok_or_else(|| Error::new("UNKNOWN_METHOD", "Unknown API method"))?;
                execute(m, s, method, true, false).await
            }
            _ => unreachable!(),
        },
        Some(("exec", s)) => {
            let method = catalog
                .find("environment/control/rest/execcmdbyid")
                .unwrap();
            execute(m, s, method, false, false).await
        }
        Some((group, sub)) => {
            let (name, s) = sub.subcommand().unwrap();
            let path = cli::friendly_groups()
                .into_iter()
                .find(|g| g.0 == group)
                .and_then(|g| g.2.into_iter().find(|c| c.0 == name).map(|c| c.1))
                .ok_or_else(|| Error::new("UNKNOWN_METHOD", "Unknown convenience command"))?;
            let method = catalog.find(path).ok_or_else(|| {
                Error::new(
                    "METHOD_UNAVAILABLE",
                    "Convenience command is absent from the selected catalog",
                )
            })?;
            execute(m, s, method, false, group == "logs" && name == "watch").await
        }
        _ => unreachable!(),
    }
}

fn run_profile(m: &ArgMatches, sub: &ArgMatches) -> Result<()> {
    let mut conf = config::Config::load()?;
    match sub.subcommand() {
        Some(("list",_))=>emit(json!(conf.profiles.iter().map(|(name,p)|json!({"name":name,"platform":p.platform,"default":conf.default_profile.as_deref()==Some(name),"credential_store":p.credential_store,"credential_kind":p.credential_kind,"environment":p.environment,"project":p.project,"node":p.node})).collect::<Vec<_>>()),m,json!({})),
        Some(("use",s))=>{let name=str_arg(s,"name").unwrap();if !conf.profiles.contains_key(name){return Err(Error::new("INVALID_INPUT","Unknown profile"));}conf.default_profile=Some(name.into());conf.save()?;emit(json!({"default_profile":name}),m,json!({}))},
        Some(("set",s))=>{let name=profile_name(m);let p=conf.profiles.get_mut(&name).ok_or_else(||Error::new("NOT_AUTHENTICATED","Unknown profile"))?;if flag(s,"clear"){p.environment=None;p.project=None;p.node=None;}else{if let Some(v)=str_arg(m,"env"){p.environment=Some(v.into());}
if let Some(v)=str_arg(m,"project"){p.project=Some(v.into());}
if let Some(v)=str_arg(m,"node"){p.node=Some(v.into());}}conf.save()?;emit(json!({"profile":name,"saved":true}),m,json!({}))},
        _=>unreachable!(),
    }
}

fn refresh(m: &ArgMatches, s: &ArgMatches, catalog: &Catalog) -> Result<()> {
    let path = Path::new(str_arg(s, "file").unwrap());
    let version = str_arg(s, "platform-version").unwrap();
    let mut new = catalog::normalize_provider(
        &read_text(path)?,
        version,
        &format!("Explicit provider export ({version})"),
    )?;
    for method in &mut new.methods {
        if let Some(old) = catalog.find(&method.path) {
            method.public = old.public;
        }
    }
    let old: std::collections::BTreeSet<_> =
        catalog.methods.iter().map(|m| m.path.as_str()).collect();
    let next: std::collections::BTreeSet<_> = new.methods.iter().map(|m| m.path.as_str()).collect();
    let added: Vec<_> = next.difference(&old).copied().collect();
    let removed: Vec<_> = old.difference(&next).copied().collect();
    let changed: Vec<_> = new
        .methods
        .iter()
        .filter(|method| {
            catalog.find(&method.path).is_some_and(|old| {
                serde_json::to_value(&old.parameters).ok()
                    != serde_json::to_value(&method.parameters).ok()
            })
        })
        .map(|m| m.path.as_str())
        .collect();
    auth::confirm(
        &format!(
            "Replace catalog for profile {}: +{}, -{}, {} changed methods?",
            profile_name(m),
            added.len(),
            removed.len(),
            changed.len()
        ),
        flag(m, "yes"),
        can_prompt(m),
    )?;
    config::atomic_private_write(
        &config::home()?
            .join("catalogs")
            .join(format!("{}.json", profile_name(m))),
        &serde_json::to_vec(&new)?,
        true,
    )?;
    emit(
        json!({"catalog":new.stats(),"added":added,"removed":removed,"changed":changed}),
        m,
        json!({}),
    )
}

async fn run_auth(m: &ArgMatches, sub: &ArgMatches) -> Result<()> {
    let name = profile_name(m);
    config::validate_profile(&name)?;
    match sub.subcommand() {
        Some(("import", s)) => {
            let sources = if str_arg(s, "from").is_none() || flag(s, "list") {
                blocking(auth::discover).await?
            } else {
                vec![]
            };
            if flag(s, "list") {
                return emit(serde_json::to_value(sources)?, m, json!({"imported":false}));
            }
            let source = if let Some(source) = str_arg(s, "from") {
                source.to_string()
            } else {
                match sources.as_slice() {
                    [source] => source.source.clone(),
                    [] => {
                        return Err(Error::new(
                            "NOT_AUTHENTICATED",
                            "No installed Jelastic session found",
                        )
                        .hint("Supply --from <properties-path> or --from wsl:Ubuntu"));
                    }
                    _ => {
                        return Err(Error::new(
                            "AMBIGUOUS_TARGET",
                            "Multiple legacy configurations found",
                        )
                        .hint("Run vap auth import --list, then select one with --from"));
                    }
                }
            };
            let source_copy = source.clone();
            let (platform, token) = blocking(move || auth::read_legacy(&source_copy)).await?;
            let store = str_arg(s, "store").unwrap().to_string();
            auth::confirm(
                &format!(
                    "Import session from {source} for {platform} into profile {name} ({store})?"
                ),
                flag(m, "yes"),
                can_prompt(m),
            )?;
            let data = blocking(move || {
                auth::save_profile(&name, platform, &token, &store, "session", Some(source))
            })
            .await?;
            emit(
                data,
                m,
                json!({"warning":"Imported sessions expire and may also be used by the old CLI"}),
            )
        }
        Some(("login", s)) => {
            let platform = config::platform(
                &str_arg(m, "platform")
                    .map(String::from)
                    .or_else(|| std::env::var("VAP_PLATFORM").ok())
                    .ok_or_else(|| {
                        Error::new("INVALID_INPUT", "Login requires --platform or VAP_PLATFORM")
                    })?,
            )?;
            let password_login = flag(s, "password-login");
            if flag(s, "password-stdin") && !password_login {
                return Err(Error::new(
                    "INVALID_INPUT",
                    "--password-stdin requires --password-login",
                ));
            }
            if password_login && (flag(s, "token-stdin") || str_arg(s, "token-file").is_some()) {
                return Err(Error::new(
                    "INVALID_INPUT",
                    "Choose PAT or password authentication",
                ));
            }
            let token = if password_login {
                let email = str_arg(s, "email").ok_or_else(|| {
                    Error::new("INVALID_INPUT", "Password login requires --email")
                })?;
                let password = auth::read_password(flag(s, "password-stdin"), can_prompt(m))?;
                let params = json!({"login":email,"password":password.as_str()})
                    .as_object()
                    .unwrap()
                    .clone();
                let client = transport::Client::new(
                    &platform,
                    Duration::from_secs(num(m, "timeout")),
                    str_arg(m, "ca").map(Path::new),
                )?;
                let response = transport::payload(
                    client
                        .call("users/authentication/rest/signin", &params, None, false)
                        .await?,
                );
                config::validate_token(response["session"].as_str().ok_or_else(||Error::new("INVALID_RESPONSE","Sign-in response has no session; use a PAT for accounts requiring 2FA"))?.into())?
            } else if let Ok(token) = std::env::var("VAP_TOKEN") {
                if flag(s, "token-stdin") || str_arg(s, "token-file").is_some() {
                    return Err(Error::new(
                        "INVALID_INPUT",
                        "Choose VAP_TOKEN or an explicit token source",
                    ));
                }
                config::validate_token(token)?
            } else {
                auth::read_secret(
                    flag(s, "token-stdin"),
                    str_arg(s, "token-file").map(Path::new),
                    can_prompt(m),
                    "Personal access token: ",
                )?
            };
            let store = str_arg(s, "store").unwrap().to_string();
            if config::Config::load()?.profiles.contains_key(&name) {
                auth::confirm(
                    &format!("Replace credentials for profile {name}?"),
                    flag(m, "yes"),
                    can_prompt(m),
                )?;
            }
            let data = blocking(move || {
                auth::save_profile(
                    &name,
                    platform,
                    &token,
                    &store,
                    if password_login { "session" } else { "pat" },
                    None,
                )
            })
            .await?;
            emit(data, m, json!({"validated_with_server":password_login}))
        }
        Some(("status", _)) => {
            let conf = config::Config::load()?;
            let (_, p) = conf.selected(Some(&name))?;
            let copy = p.clone();
            let available = blocking(move || config::credential(&copy).map(|_| true))
                .await
                .is_ok();
            emit(
                json!({"profile":name,"platform":p.platform,"credential_kind":p.credential_kind,"credential_store":p.credential_store,"credential_available":available,"live_validity":"not checked","imported_from":p.imported_from}),
                m,
                json!({}),
            )
        }
        Some((action, _)) if action == "logout" || action == "revoke" => {
            let mut conf = config::Config::load()?;
            let (_, p) = conf.selected(Some(&name))?;
            let p = p.clone();
            if action == "revoke" {
                if p.credential_kind != "session" {
                    return Err(Error::new(
                        "INVALID_INPUT",
                        "PAT revocation must be performed through the platform's token management",
                    ));
                }
                auth::confirm(
                    &format!(
                        "Revoke session for {}? Other clients sharing it will lose access.",
                        p.platform
                    ),
                    flag(m, "yes"),
                    can_prompt(m),
                )?;
                let copy = p.clone();
                let token = blocking(move || config::credential(&copy)).await?;
                let client = transport::Client::new(
                    &p.platform,
                    Duration::from_secs(num(m, "timeout")),
                    str_arg(m, "ca").map(Path::new),
                )?;
                client
                    .call(
                        "users/authentication/rest/signout",
                        &Map::new(),
                        Some(&token),
                        false,
                    )
                    .await?;
            }
            let copy = p.clone();
            blocking(move || config::forget(&copy)).await?;
            conf.profiles.remove(&name);
            if conf.default_profile.as_deref() == Some(&name) {
                conf.default_profile = None;
            }
            conf.save()?;
            emit(
                json!({"profile":name,"local_credential_removed":true,"server_session_revoked":action=="revoke"}),
                m,
                json!({}),
            )
        }
        _ => unreachable!(),
    }
}

fn parse_parameter(value: &str, kind: &str) -> Result<Value> {
    match kind {
        "int" | "Integer" => value
            .parse::<i32>()
            .map(|v| json!(v))
            .map_err(|_| Error::new("INVALID_INPUT", "Expected a 32-bit integer API parameter")),
        "long" | "Long" => value
            .parse::<i64>()
            .map(|v| json!(v))
            .map_err(|_| Error::new("INVALID_INPUT", "Expected a 64-bit integer API parameter")),
        "boolean" | "Boolean" => value
            .parse::<bool>()
            .map(|v| json!(v))
            .map_err(|_| Error::new("INVALID_INPUT", "Boolean parameters require true or false")),
        s if s.starts_with(['{', '[']) => Ok(serde_json::from_str(value)?),
        _ => Ok(json!(value)),
    }
}
fn inline_allowed(name: &str, value: &Value) -> Result<()> {
    if catalog::requires_file(name) || !output::secret_values(value, true).is_empty() {
        return Err(Error::new(
            "INVALID_INPUT",
            "Secret or opaque payload parameters cannot be supplied in command arguments",
        )
        .hint("Use --input file.json or --input -"));
    }
    Ok(())
}

fn parameters(
    m: &ArgMatches,
    s: &ArgMatches,
    method: &Method,
    generated: bool,
) -> Result<Map<String, Value>> {
    let mut params = if let Some(path) = str_arg(m, "input") {
        serde_json::from_str::<Value>(&read_text(Path::new(path))?)?
            .as_object()
            .cloned()
            .ok_or_else(|| {
                Error::new(
                    "INVALID_INPUT",
                    "Input must be a JSON object of API parameters",
                )
            })?
    } else {
        Map::new()
    };
    if let Some(values) = m.get_many::<String>("param") {
        for value in values {
            let (name, value) = value
                .split_once('=')
                .ok_or_else(|| Error::new("INVALID_INPUT", "Use --param NAME=VALUE"))?;
            let p = method
                .parameters
                .iter()
                .find(|p| p.name == name || p.aliases.iter().any(|a| a == name));
            let parsed = parse_parameter(value, p.map(|p| p.kind.as_str()).unwrap_or("String"))?;
            inline_allowed(p.map(|p| p.name.as_str()).unwrap_or(name), &parsed)?;
            params.insert(name.into(), parsed);
        }
    }
    if generated {
        for p in &method.parameters {
            if let Some(value) = str_arg(s, &format!("api.{}", p.name)) {
                let parsed = parse_parameter(value, &p.kind)?;
                inline_allowed(&p.name, &parsed)?;
                params.insert(p.name.clone(), parsed);
            }
        }
    }
    for p in &method.parameters {
        for alias in &p.aliases {
            if alias == &p.name || method.parameters.iter().any(|other| &other.name == alias) {
                continue;
            }
            if let Some(v) = params.remove(alias) {
                if let Some(existing) = params.get(&p.name) {
                    if existing != &v {
                        return Err(Error::new(
                            "INVALID_INPUT",
                            "Conflicting canonical and alias parameter values",
                        ));
                    }
                } else {
                    params.insert(p.name.clone(), v);
                }
            }
        }
    }
    if params
        .keys()
        .any(|k| k.eq_ignore_ascii_case("session") || k.eq_ignore_ascii_case("token"))
    {
        return Err(Error::new(
            "INVALID_INPUT",
            "Session is injected from the profile; use VAP_TOKEN to override authentication",
        ));
    }
    let conf = config::Config::load()?;
    let name = profile_name(m);
    let selected = conf.selected(Some(&name)).ok().map(|(_, p)| p);
    for (target, saved) in [
        ("env", selected.and_then(|p| p.environment.as_deref())),
        ("project", selected.and_then(|p| p.project.as_deref())),
        ("node", selected.and_then(|p| p.node.as_deref())),
    ] {
        let explicit = str_arg(m, target);
        let names: &[&str] = match target {
            "env" => &["envName", "srcEnvName"],
            "project" => {
                if explicit.is_some() {
                    &["projectId", "project"]
                } else {
                    &["projectId"]
                }
            }
            _ => &["nodeId", "nodeid"],
        };
        let parameter = names
            .iter()
            .find_map(|name| method.parameters.iter().find(|p| p.name == *name))
            .or_else(|| {
                if target == "env" {
                    method.parameters.iter().find(|p| {
                        p.name == "appid"
                            && (method.service == "tracking"
                                || p.aliases
                                    .iter()
                                    .any(|a| a.eq_ignore_ascii_case("envName") || a == "domain"))
                    })
                } else {
                    None
                }
            });
        if explicit.is_some() && parameter.is_none() {
            return Err(Error::new(
                "INVALID_INPUT",
                format!("--{target} does not apply to this method"),
            )
            .hint("Inspect vap api describe; use exact API parameters in --input"));
        }
        if let (Some(value), Some(p)) = (explicit.or(saved), parameter) {
            let value = parse_parameter(value, &p.kind)?;
            if explicit.is_some() {
                params.insert(p.name.clone(), value);
            } else {
                params.entry(p.name.clone()).or_insert(value);
            }
        }
    }
    if let Some(name) = str_arg(s, "name") {
        if method.name == "cloneenv" {
            params.insert("dstEnvName".into(), json!(name));
        }
        if method.name == "createenvironment" {
            let env = params.entry("env").or_insert(json!({}));
            let env = env.as_object_mut().ok_or_else(|| {
                Error::new("INVALID_INPUT", "CreateEnvironment env must be an object")
            })?;
            env.insert("shortdomain".into(), json!(name));
        }
    }
    if let Some(disk) = s.try_get_one::<u32>("disk-gb").ok().flatten() {
        let nodes = params
            .get_mut("nodes")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| {
                Error::new(
                    "INVALID_INPUT",
                    "--disk-gb requires a nodes array in --input",
                )
            })?;
        for node in nodes {
            node.as_object_mut()
                .ok_or_else(|| Error::new("INVALID_INPUT", "Each node must be an object"))?
                .insert("diskLimit".into(), json!(disk));
        }
    }
    if let Some(path) = str_arg(s, "command-file") {
        params.insert(
            "commandList".into(),
            json!([{"command":read_text(Path::new(path))?}]),
        );
    }
    if method.name == "readlog" {
        params.entry("count").or_insert(json!(100));
    }
    if !method.parameters.is_empty() {
        for (key, value) in &params {
            let p = method
                .parameters
                .iter()
                .find(|p| p.name == *key)
                .ok_or_else(|| {
                    Error::new(
                        "INVALID_INPUT",
                        "Unknown API parameter; inspect vap api describe",
                    )
                })?;
            if value.is_null() {
                return Err(Error::new(
                    "INVALID_INPUT",
                    format!("Parameter {} cannot be null; omit optional values", p.name),
                ));
            }
            match p.kind.as_str() {
                "int" | "Integer" => {
                    if value.as_i64().and_then(|v| i32::try_from(v).ok()).is_none()
                        && value.as_str().is_none_or(|v| v.parse::<i32>().is_err())
                    {
                        return Err(Error::new(
                            "INVALID_INPUT",
                            format!("Parameter {} requires a 32-bit integer", p.name),
                        ));
                    }
                }
                "boolean" | "Boolean"
                    if !value.is_boolean()
                        && !value
                            .as_str()
                            .is_some_and(|v| matches!(v, "true" | "false")) =>
                {
                    return Err(Error::new(
                        "INVALID_INPUT",
                        format!("Parameter {} requires a boolean", p.name),
                    ));
                }
                _ => {}
            }
        }
        for p in method
            .parameters
            .iter()
            .filter(|p| p.required && p.name != "session")
        {
            if !params.contains_key(&p.name) {
                return Err(Error::new(
                    "INVALID_INPUT",
                    format!("Missing required API parameter: {}", p.name),
                )
                .hint(format!(
                    "Run vap api describe {} and supply --input file.json or a non-secret flag",
                    method.path
                )));
            }
        }
    }
    Ok(params)
}

async fn execute(
    m: &ArgMatches,
    s: &ArgMatches,
    method: &Method,
    generated: bool,
    watch: bool,
) -> Result<()> {
    if flag(m, "read-only") && !method.read_only() {
        return Err(Error::new(
            "PERMISSION_DENIED",
            "Operation is modifying or not audited as read-only",
        ));
    }
    let params = parameters(m, s, method, generated)?;
    let conf = config::Config::load()?;
    let name = profile_name(m);
    let selected = conf.selected(Some(&name)).ok().map(|(_, p)| p.clone());
    let platform = str_arg(m, "platform")
        .map(String::from)
        .or_else(|| std::env::var("VAP_PLATFORM").ok())
        .or_else(|| selected.as_ref().map(|p| p.platform.clone()))
        .map(|p| config::platform(&p))
        .transpose()?;
    let mut secrets =
        output::secret_values(&Value::Object(params.clone()), method.namespace != "s3");
    if flag(m, "preview") {
        let mut data = transport::preview(method, &params, platform.as_deref());
        output::redact(&mut data, &secrets, method.namespace != "s3");
        return emit(data, m, json!({"sent":false}));
    }
    let platform = platform.ok_or_else(|| {
        Error::new(
            "NOT_AUTHENTICATED",
            "Set --platform / VAP_PLATFORM or configure a profile",
        )
    })?;
    let token = if std::env::var_os("VAP_TOKEN").is_some() {
        config::validate_token(
            std::env::var("VAP_TOKEN")
                .map_err(|_| Error::new("INVALID_INPUT", "VAP_TOKEN is not UTF-8"))?,
        )?
    } else if let Some(path) = std::env::var_os("VAP_TOKEN_FILE") {
        config::validate_token(std::fs::read_to_string(path)?)?
    } else {
        let p = selected.ok_or_else(|| {
            Error::new(
                "NOT_AUTHENTICATED",
                "No credential; run vap auth import/login or supply VAP_TOKEN",
            )
        })?;
        if p.platform != platform {
            return Err(Error::new(
                "PERMISSION_DENIED",
                "Refusing to send a stored credential to another platform",
            )
            .hint("Create a separate profile or explicitly supply VAP_TOKEN"));
        }
        blocking(move || config::credential(&p)).await?
    };
    secrets.push(token.to_string());
    if !method.read_only() {
        auth::confirm(
            &format!(
                "POST {} on {} (profile {name}); environment {}, project {}, node {}?",
                method.path,
                platform,
                params
                    .get("envName")
                    .or_else(|| params.get("srcEnvName"))
                    .unwrap_or(&Value::Null),
                params.get("projectId").unwrap_or(&Value::Null),
                params
                    .get("nodeId")
                    .or_else(|| params.get("nodeid"))
                    .unwrap_or(&Value::Null)
            ),
            flag(m, "yes"),
            can_prompt(m),
        )?;
    }
    let client = transport::Client::new(
        &platform,
        Duration::from_secs(num(m, "timeout")),
        str_arg(m, "ca").map(Path::new),
    )?;
    let wait_target = if flag(m, "wait") {
        Some(wait_target(method, &params)?)
    } else {
        None
    };
    let started = tokio::time::Instant::now();
    let progress =
        crate::progress::Progress::new(format!("{} on {}", method.path, platform), flag(m, "json"));
    if let Some(path) = str_arg(m, "output")
        && Path::new(path).exists()
    {
        return Err(Error::new(
            "INVALID_INPUT",
            "Output already exists; refusing to send the API request",
        ));
    }
    let response = client
        .call(&method.path, &params, Some(&token), method.read_only())
        .await?;
    if let Some(path) = str_arg(m, "output") {
        config::atomic_private_write(
            Path::new(path),
            &serde_json::to_vec_pretty(&response)?,
            false,
        ).map_err(|_|Error::new("OUTPUT_SAVE_FAILED","API succeeded but its response could not be saved; do not repeat a modifying request").hint("Inspect the resource/operation state and choose a new output path"))?;
    }
    let mut response = transport::payload(response);
    if let Some((env, target)) = wait_target {
        response = wait_env(
            &client,
            &token,
            &env,
            target,
            started,
            Duration::from_secs(num(m, "timeout")),
            Duration::from_secs(num(m, "poll-interval")),
        )
        .await?;
    }
    drop(progress);
    let mut data = if method.name == "getenvs" {
        response
            .get("infos")
            .or_else(|| response.get("environments"))
            .cloned()
            .unwrap_or(response)
    } else if m.subcommand_name() == Some("node") && method.name == "getenvinfo" {
        response.get("nodes").cloned().unwrap_or(response)
    } else {
        response
    };
    output::redact(&mut data, &secrets, method.namespace != "s3");
    if watch {
        let samples = s.get_one::<u32>("samples").copied().unwrap_or(10);
        let mut previous = None;
        for sample in 0..samples {
            if sample > 0 {
                tokio::time::sleep(Duration::from_secs(num(m, "poll-interval"))).await;
                data = transport::payload(
                    client
                        .call(&method.path, &params, Some(&token), true)
                        .await?,
                );
                output::redact(&mut data, &secrets, method.namespace != "s3");
            }
            if previous.as_ref() != Some(&data) {
                emit(
                    data.clone(),
                    m,
                    json!({"event":"log_snapshot","sample":sample+1,"samples":samples}),
                )?;
                previous = Some(data.clone());
            }
            if started.elapsed() >= Duration::from_secs(num(m, "timeout")) {
                return Err(Error::new("TIMEOUT", "Log watch deadline reached"));
            }
        }
        return Ok(());
    }
    emit(
        data,
        m,
        json!({"method":method.path,"platform":platform,"profile":name,"response_saved":str_arg(m,"output").is_some(),"secrets_redacted":true}),
    )
}

#[derive(Clone, Copy)]
enum EnvTarget {
    Running,
    Stopped,
    Absent,
}
fn wait_target(method: &Method, params: &Map<String, Value>) -> Result<(String, EnvTarget)> {
    if method.namespace != "environment" || method.service != "control" {
        return Err(Error::new(
            "INVALID_INPUT",
            "--wait requires a supported environment lifecycle method",
        ));
    }
    let (name, target) = match method.name.as_str() {
        "createenvironment" => (
            params
                .get("env")
                .and_then(|e| e.get("shortdomain"))
                .and_then(Value::as_str),
            EnvTarget::Running,
        ),
        "cloneenv" => (
            params.get("dstEnvName").and_then(Value::as_str),
            EnvTarget::Running,
        ),
        "startenv" => (
            params.get("envName").and_then(Value::as_str),
            EnvTarget::Running,
        ),
        "stopenv" => (
            params.get("envName").and_then(Value::as_str),
            EnvTarget::Stopped,
        ),
        "deleteenv" => (
            params.get("envName").and_then(Value::as_str),
            EnvTarget::Absent,
        ),
        _ => {
            return Err(Error::new(
                "INVALID_INPUT",
                "--wait is supported for environment create/clone/start/stop/delete; inspect other operations with vap operations",
            ));
        }
    };
    Ok((
        name.ok_or_else(|| {
            Error::new(
                "INVALID_INPUT",
                "--wait requires an explicit environment name",
            )
        })?
        .into(),
        target,
    ))
}
fn env_finished(value: &Value, env: &str, target: EnvTarget) -> Result<bool> {
    let infos = value
        .get("infos")
        .or_else(|| value.get("environments"))
        .and_then(Value::as_array)
        .ok_or_else(|| {
            Error::new(
                "INVALID_RESPONSE",
                "GetEnvs response has no environment collection; cannot establish completion",
            )
        })?;
    let info = infos.iter().find(|i| {
        i.pointer("/env/envName")
            .or_else(|| i.get("envName"))
            .and_then(Value::as_str)
            == Some(env)
    });
    let Some(info) = info else {
        return Ok(matches!(target, EnvTarget::Absent));
    };
    let state = info.get("env").unwrap_or(info);
    let status = state.get("status").and_then(Value::as_i64);
    Ok(match target {
        EnvTarget::Absent => false,
        EnvTarget::Stopped => status == Some(2),
        EnvTarget::Running => {
            status == Some(1)
                && info
                    .get("nodes")
                    .and_then(Value::as_array)
                    .is_some_and(|nodes| !nodes.is_empty())
        }
    })
}
async fn wait_env(
    client: &transport::Client,
    token: &str,
    env: &str,
    target: EnvTarget,
    started: tokio::time::Instant,
    timeout: Duration,
    interval: Duration,
) -> Result<Value> {
    let future = async {
        loop {
            let value = transport::payload(
                client
                    .call(
                        "environment/control/rest/getenvs",
                        &Map::new(),
                        Some(token),
                        true,
                    )
                    .await?,
            );
            if env_finished(&value, env, target)? {
                return Ok(json!({"environment":env,"completed":true,"state":value}));
            }
            tokio::time::sleep(interval).await;
        }
    };
    tokio::time::timeout_at(started + timeout, future)
        .await
        .map_err(|_| {
            Error::new(
                "OUTCOME_UNKNOWN",
                "Wait deadline reached; the server operation may continue",
            )
            .hint("Inspect vap operations list and vap env list before retrying")
        })?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn provisioning_ack_is_not_running() {
        let v = json!({"infos":[{"env":{"envName":"new","status":6},"nodes":[]}]});
        assert!(!env_finished(&v, "new", EnvTarget::Running).unwrap());
        let v = json!({"infos":[{"env":{"envName":"new","status":1},"nodes":[{"id":1}]}]});
        assert!(env_finished(&v, "new", EnvTarget::Running).unwrap());
    }
    #[test]
    fn absence_requires_valid_collection() {
        assert!(env_finished(&json!({}), "x", EnvTarget::Absent).is_err());
        assert!(env_finished(&json!({"infos":[]}), "x", EnvTarget::Absent).unwrap());
    }
    #[test]
    fn inline_nested_secrets_rejected() {
        assert!(inline_allowed("options", &json!({"password":"secret"})).is_err());
        assert!(inline_allowed("commandList", &json!("echo hidden")).is_err());
    }
}
