use crate::{
    auth,
    catalog::{self, Catalog},
    cli, config,
    context::{Bootstrap, ExecutionContext},
    engine::{Engine, ExecutionResult},
    error::{Error, Result},
    invocation::{Selection, flag, text as str_arg},
    output, registry,
    request::PreparedRequest,
    transport,
};
use clap::ArgMatches;
use serde_json::{Value, json};
use std::{
    io::{self, Read},
    path::Path,
    time::Duration,
};

pub async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let json_mode = args.iter().any(|s| s == "--json");
    let result=async {
        let mut bootstrap=Bootstrap::load(&args)?;
        let catalog=load_catalog(&bootstrap.profile)?;
        let mut command=cli::build(&catalog);
        let matches=match command.clone().try_get_matches_from(&args) {
            Ok(matches)=>matches,
            Err(error) if matches!(error.kind(),clap::error::ErrorKind::DisplayHelp|clap::error::ErrorKind::DisplayVersion)
                || (!json_mode && error.kind()==clap::error::ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand)=> {
                error.print().map_err(Error::from)?;
                return Ok(());
            }
            Err(_)=>return Err(Error::new("INVALID_INPUT","Invalid command or option")
                .hint("Use vap --help or the command's --help; secrets must use file/stdin")),
        };
        tokio::select! {
            result=run(&matches,&catalog,&mut command,&mut bootstrap)=>result,
            _=tokio::signal::ctrl_c()=>Err(Error::new("INTERRUPTED","Stopped waiting; any server operation may continue")
                .hint("Inspect operations and resource state before retrying")),
        }
    }.await;
    if let Err(error) = result {
        output::failure(&error, json_mode);
        std::process::exit(error.exit_code());
    }
}

fn num(m: &ArgMatches, name: &str) -> u64 {
    m.get_one::<u64>(name).copied().unwrap_or(600)
}
fn can_prompt(m: &ArgMatches) -> bool {
    auth::interactive(flag(m, "json"), flag(m, "non-interactive"))
}
fn load_catalog(name: &str) -> Result<Catalog> {
    config::validate_profile(name)?;
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

async fn run(
    m: &ArgMatches,
    catalog: &Catalog,
    command: &mut clap::Command,
    bootstrap: &mut Bootstrap,
) -> Result<()> {
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
            let conf = &bootstrap.config;
            emit(
                json!({"version":env!("CARGO_PKG_VERSION"),"os":std::env::consts::OS,"arch":std::env::consts::ARCH,"config_directory":config::home()?,"profiles":conf.profiles.keys().collect::<Vec<_>>(),"catalog":catalog.stats(),"http_backend":crate::http_client::NAME,"tls":crate::http_client::TLS,"telemetry":false,"automatic_updates":false}),
                m,
                json!({}),
            )
        }
        Some(("profile", sub)) => run_profile(m, sub, &mut bootstrap.config, &bootstrap.profile),
        Some(("auth", sub)) => run_auth(m, sub, bootstrap).await,
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
                    refresh(m, s, catalog, &bootstrap.profile)
                } else {
                    emit(
                        serde_json::to_value(catalog)?,
                        m,
                        json!({"catalog":catalog.stats()}),
                    )
                }
            }
            _ => execute(m, catalog, bootstrap).await,
        },
        Some(_) => execute(m, catalog, bootstrap).await,
        _ => unreachable!(),
    }
}

fn run_profile(
    m: &ArgMatches,
    sub: &ArgMatches,
    conf: &mut config::Config,
    profile: &str,
) -> Result<()> {
    match sub.subcommand() {
        Some(("list", _)) => {
            let profiles:Vec<_>=conf.profiles.iter().map(|(name,p)|json!({"name":name,"platform":p.platform,"default":conf.default_profile.as_deref()==Some(name),"credential_store":p.credential_store,"credential_kind":p.credential_kind,"environment":p.environment,"project":p.project,"node":p.node})).collect();
            emit(json!(profiles), m, json!({}))
        }
        Some(("use", sub)) => {
            let name = str_arg(sub, "name").unwrap();
            if !conf.profiles.contains_key(name) {
                return Err(Error::new("INVALID_INPUT", "Unknown profile"));
            }
            conf.default_profile = Some(name.into());
            conf.save()?;
            emit(json!({"default_profile":name}), m, json!({}))
        }
        Some(("set", sub)) => {
            let selected = conf
                .profiles
                .get_mut(profile)
                .ok_or_else(|| Error::new("NOT_AUTHENTICATED", "Unknown profile"))?;
            if flag(sub, "clear") {
                selected.environment = None;
                selected.project = None;
                selected.node = None;
            } else {
                if let Some(value) = str_arg(m, "env") {
                    selected.environment = Some(value.into());
                }
                if let Some(value) = str_arg(m, "project") {
                    selected.project = Some(value.into());
                }
                if let Some(value) = str_arg(m, "node") {
                    selected.node = Some(value.into());
                }
            }
            conf.save()?;
            emit(json!({"profile":profile,"saved":true}), m, json!({}))
        }
        _ => unreachable!(),
    }
}

fn refresh(m: &ArgMatches, s: &ArgMatches, catalog: &Catalog, profile: &str) -> Result<()> {
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
            profile,
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
            .join(format!("{profile}.json")),
        &serde_json::to_vec(&new)?,
        true,
    )?;
    emit(
        json!({"catalog":new.stats(),"added":added,"removed":removed,"changed":changed}),
        m,
        json!({}),
    )
}

async fn run_auth(m: &ArgMatches, sub: &ArgMatches, bootstrap: &Bootstrap) -> Result<()> {
    let name = bootstrap.profile.clone();
    let conf = bootstrap.config.clone();
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
                auth::save_profile(
                    conf,
                    &name,
                    platform,
                    &token,
                    &store,
                    "session",
                    Some(source),
                )
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
                        .call(
                            &PreparedRequest::new("users/authentication/rest/signin", params)?,
                            None,
                            tokio::time::Instant::now() + Duration::from_secs(num(m, "timeout")),
                        )
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
            if bootstrap.config.profiles.contains_key(&name) {
                auth::confirm(
                    &format!("Replace credentials for profile {name}?"),
                    flag(m, "yes"),
                    can_prompt(m),
                )?;
            }
            let data = blocking(move || {
                auth::save_profile(
                    conf,
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
            let conf = &bootstrap.config;
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
            let mut conf = bootstrap.config.clone();
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
                        &PreparedRequest::new(
                            "users/authentication/rest/signout",
                            serde_json::Map::new(),
                        )?,
                        Some(&token),
                        tokio::time::Instant::now() + Duration::from_secs(num(m, "timeout")),
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

async fn execute(m: &ArgMatches, catalog: &Catalog, bootstrap: &Bootstrap) -> Result<()> {
    let selection = Selection::resolve(m, catalog)?;
    let context = ExecutionContext::new(
        &bootstrap.config,
        &bootstrap.profile,
        str_arg(m, "platform").map(String::from),
        Duration::from_secs(num(m, "timeout")),
        Duration::from_secs(num(m, "poll-interval")),
        str_arg(m, "ca").map(Into::into),
    )?;
    let invocation = selection.prepare(m, context.targets.clone(), |path| {
        read_text(Path::new(path))
    })?;
    let request = &invocation.request;
    let mut secrets = output::secret_values(
        &Value::Object(request.params().clone()),
        !request.path().starts_with("s3/"),
    );
    if flag(m, "preview") {
        let mut data = transport::preview(request, context.platform.as_deref());
        output::redact(&mut data, &secrets, !request.path().starts_with("s3/"));
        return emit(data, m, json!({"sent":false}));
    }
    let authorized = context.authorize(invocation.authentication).await?;
    let context = authorized.context;
    let token = authorized.credential;
    if let Some(token) = &token {
        secrets.push(token.to_string());
    }
    let platform = context
        .platform
        .as_deref()
        .ok_or_else(|| Error::new("NOT_AUTHENTICATED", "Missing platform"))?;
    if !request.policy().read_only() {
        auth::confirm(
            &format!(
                "POST {} on {} (profile {}); environment {}, project {}, node {}?",
                request.path(),
                platform,
                context.profile_name,
                request
                    .params()
                    .get("envName")
                    .or_else(|| request.params().get("srcEnvName"))
                    .unwrap_or(&Value::Null),
                request.params().get("projectId").unwrap_or(&Value::Null),
                request
                    .params()
                    .get("nodeId")
                    .or_else(|| request.params().get("nodeid"))
                    .unwrap_or(&Value::Null)
            ),
            flag(m, "yes"),
            can_prompt(m),
        )?;
    }
    if str_arg(m, "output").is_some_and(|p| Path::new(p).exists()) {
        return Err(Error::new(
            "INVALID_INPUT",
            "Output already exists; refusing to send the API request",
        ));
    }
    let client = transport::Client::new(platform, context.timeout, context.ca.as_deref())?;
    let engine = Engine {
        client: &client,
        credential: token.as_deref().map(|s| s.as_str()),
        timeout: context.timeout,
        interval: context.interval,
    };
    let progress = crate::progress::Progress::new(
        format!("{} on {}", request.path(), platform),
        flag(m, "json"),
    );
    let result=engine.run(request,invocation.mode,|response| {
        if let Some(path)=str_arg(m,"output") {
            config::atomic_private_write(Path::new(path),&serde_json::to_vec_pretty(response)?,false).map_err(|_|Error::new("OUTPUT_SAVE_FAILED","API succeeded but its response could not be saved; do not repeat a modifying request").hint("Inspect the resource/operation state and choose a new output path"))?;
        } Ok(())
    },|event| {
        let mut data=event.data;
        output::redact(&mut data,&secrets,!request.path().starts_with("s3/"));
        emit(data,m,event.meta)
    }).await?;
    drop(progress);
    if let ExecutionResult::Response(response) = result {
        let mut data = registry::project(request.path(), invocation.spec, response);
        output::redact(&mut data, &secrets, !request.path().starts_with("s3/"));
        emit(
            data,
            m,
            json!({"method":request.path(),"platform":platform,"profile":context.profile_name,"response_saved":str_arg(m,"output").is_some(),"secrets_redacted":true}),
        )?;
    }
    Ok(())
}
