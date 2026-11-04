use crate::catalog::{Catalog, Method, flag_name, requires_file, sensitive};
use clap::{Arg, ArgAction, Command};
use std::collections::{BTreeMap, BTreeSet};

pub const GLOBALS: &[&str] = &[
    "profile",
    "platform",
    "env",
    "project",
    "node",
    "json",
    "non-interactive",
    "yes",
    "input",
    "param",
    "output",
    "fields",
    "limit",
    "preview",
    "read-only",
    "wait",
    "timeout",
    "poll-interval",
    "ca",
    "help",
    "version",
];
fn flag(name: &'static str, help: &'static str) -> Arg {
    Arg::new(name)
        .long(name)
        .help(help)
        .action(ArgAction::SetTrue)
}
fn value(name: &'static str, help: &'static str) -> Arg {
    Arg::new(name).long(name).help(help).value_name("VALUE")
}

pub fn build(catalog: &Catalog) -> Command {
    let mut root=Command::new("vap").version(env!("CARGO_PKG_VERSION")).about("Virtuozzo API for people and agents").long_about("One binary. Offline API discovery. Explicit targets. No secrets in command arguments.\nStart with vap auth import --list or vap api search <term>.").subcommand_required(true).arg_required_else_help(true);
    for arg in [
        flag("json", "Versioned JSON output; never prompt"),
        flag("non-interactive", "Never prompt"),
        flag("yes", "Confirm a reviewed change"),
        flag(
            "preview",
            "Construct and display a redacted request without sending it",
        ),
        flag("read-only", "Reject modifying or unaudited API operations"),
        flag(
            "wait",
            "Wait for a supported asynchronous operation to finish",
        ),
        value("profile", "Configuration profile"),
        value("platform", "HTTPS platform origin (or VAP_PLATFORM)"),
        value("env", "Environment name"),
        value(
            "project",
            "Cloud Management engineProjectId (or API project)",
        ),
        value("node", "Node ID"),
        value("input", "JSON parameters from a file; - reads stdin"),
        value(
            "output",
            "Save the complete API response privately; refuses overwrite",
        ),
        value("param", "Non-secret API parameter NAME=VALUE").action(ArgAction::Append),
        value("fields", "Comma-separated output fields").value_delimiter(','),
        value("limit", "Maximum displayed items; truncation is explicit")
            .value_parser(clap::value_parser!(usize)),
        value("timeout", "Request/wait deadline in seconds")
            .default_value("600")
            .value_parser(clap::value_parser!(u64).range(1..)),
        value("poll-interval", "Wait/watch interval in seconds")
            .default_value("5")
            .value_parser(clap::value_parser!(u64).range(1..)),
        value("ca", "Additional trusted CA certificates in a PEM file"),
    ] {
        root = root.arg(arg.global(true));
    }

    let mut api = Command::new("api")
        .about("Discover and invoke the complete API catalog")
        .subcommand_required(true)
        .subcommand(
            Command::new("search")
                .about("Search offline API methods")
                .arg(Arg::new("query").required(true))
                .arg(flag("public-only", "Only public catalog methods")),
        )
        .subcommand(
            Command::new("describe")
                .about("Describe parameters, aliases, provenance and permissions caveat")
                .arg(Arg::new("method").required(true)),
        )
        .subcommand(
            Command::new("schema")
                .about("Print the catalog or explicitly refresh it from a provider JSON export")
                .subcommand(
                    Command::new("refresh")
                        .arg(
                            value(
                                "file",
                                "Provider api.json/private_api.json or normalized catalog",
                            )
                            .required(true),
                        )
                        .arg(value("platform-version", "Source platform version").required(true)),
                ),
        )
        .subcommand(
            Command::new("call")
                .about("Call a relative REST path, including methods absent from this catalog")
                .arg(Arg::new("path").required(true)),
        );
    let mut tree = BTreeMap::<&str, BTreeMap<&str, Vec<&Method>>>::new();
    for m in &catalog.methods {
        tree.entry(&m.namespace)
            .or_default()
            .entry(&m.service)
            .or_default()
            .push(m);
    }
    for (namespace, services) in tree {
        let mut ns = Command::new(namespace.to_string()).subcommand_required(true);
        for (service, methods) in services {
            let mut svc = Command::new(service.to_string()).subcommand_required(true);
            for m in methods {
                let mut command=Command::new(m.name.clone()).about(if m.public{"Public API method"}else{"Extended API method; platform permissions required"}).after_help(format!("POST /1.0/{}\nUse --input file.json for structured or secret parameters.\nAuthentication is injected from your profile; never pass session in argv.\nSee: {}",m.path,m.doc_url.as_deref().unwrap_or("vap api describe")));
                let mut used: BTreeSet<String> = GLOBALS.iter().map(|s| s.to_string()).collect();
                // Reserve all canonical names before aliases so alternate spellings cannot shadow parameters.
                used.extend(m.parameters.iter().map(|p| flag_name(&p.name)));
                for p in &m.parameters {
                    let long = flag_name(&p.name);
                    if requires_file(&p.name) || GLOBALS.contains(&long.as_str()) {
                        continue;
                    }
                    let mut arg = Arg::new(format!("api.{}", p.name))
                        .long(long)
                        .value_name(if p.kind.starts_with(['{', '[']) {
                            "JSON".to_string()
                        } else {
                            p.kind.clone()
                        })
                        .help(format!(
                            "{}{}",
                            if p.required {
                                "Required API parameter. "
                            } else {
                                "Optional API parameter. "
                            },
                            if p.kind.starts_with(['{', '[']) {
                                "JSON; a file/stdin is recommended"
                            } else {
                                ""
                            }
                        ));
                    for alias in &p.aliases {
                        let a = flag_name(alias);
                        if !sensitive(alias) && used.insert(a.clone()) {
                            arg = arg.visible_alias(a);
                        }
                    }
                    command = command.arg(arg);
                }
                svc = svc.subcommand(command);
            }
            ns = ns.subcommand(svc);
        }
        api = api.subcommand(ns);
    }
    root = root.subcommand(api);
    let auth = Command::new("auth")
        .about("Login, import an existing session or manage credentials")
        .subcommand_required(true)
        .subcommand(
            Command::new("import")
                .about("Review and import installed Jelastic/VAP session")
                .arg(flag("list", "List discovered sources without importing"))
                .arg(value("from", "Legacy properties path or wsl:DISTRIBUTION"))
                .arg(
                    value(
                        "store",
                        "Credential storage; file must be explicitly selected",
                    )
                    .value_parser(["keychain", "file"])
                    .default_value("keychain"),
                ),
        )
        .subcommand(
            Command::new("login")
                .about("Store a PAT, or sign in with an interactively entered password")
                .arg(flag("token-stdin", "Read PAT from stdin"))
                .arg(value("token-file", "Read PAT from a file"))
                .arg(flag(
                    "password-login",
                    "Use password authentication instead of a PAT",
                ))
                .arg(flag(
                    "password-stdin",
                    "Read password from stdin; requires --password-login",
                ))
                .arg(value("email", "Account email for password login"))
                .arg(
                    value("store", "Credential storage")
                        .value_parser(["keychain", "file"])
                        .default_value("keychain"),
                ),
        )
        .subcommand(
            Command::new("status")
                .about("Show profile and credential status without revealing secrets"),
        )
        .subcommand(
            Command::new("logout").about("Remove local credentials; server session is kept"),
        )
        .subcommand(
            Command::new("revoke")
                .about("Revoke the server session, including other clients sharing it"),
        );
    root = root.subcommand(auth);
    root = root.subcommand(
        Command::new("profile")
            .subcommand_required(true)
            .subcommand(Command::new("list"))
            .subcommand(Command::new("use").arg(Arg::new("name").required(true)))
            .subcommand(
                Command::new("set")
                    .about("Save --env/--project/--node defaults; --clear removes them")
                    .arg(flag("clear", "Clear all target defaults")),
            ),
    );
    for (name, about, commands) in friendly_groups() {
        let mut group = Command::new(name).about(about).subcommand_required(true);
        for (name, path) in commands {
            let mut cmd = Command::new(name).about(format!("API: {path}"));
            if let Some(method) = catalog.find(path) {
                cmd=cmd.after_help(format!("Use --input file.json or --param NAME=VALUE.\nInspect parameters: vap api describe {path}"));
                if matches!(method.name.as_str(), "createenvironment" | "cloneenv") {
                    cmd = cmd.arg(value("name", "New environment name"));
                }
                if name == "create" && path == "environment/control/rest/createenvironment" {
                    cmd = cmd.arg(
                        value("disk-gb", "Disk size in GB, applied to every node")
                            .value_parser(clap::value_parser!(u32).range(1..)),
                    );
                }
                if name == "watch" {
                    cmd = cmd.arg(
                        value("samples", "Finite number of log snapshots")
                            .default_value("10")
                            .value_parser(clap::value_parser!(u32).range(1..)),
                    );
                }
                let _ = method;
            }
            group = group.subcommand(cmd);
        }
        root = root.subcommand(group);
    }
    root.subcommand(
        Command::new("exec")
            .about("Execute a command on an explicit node; use --input for command payloads")
            .arg(value(
                "command-file",
                "Read the command string from a file; - reads stdin",
            )),
    )
    .subcommand(
        Command::new("completion")
            .about("Generate shell completions offline")
            .arg(Arg::new("shell").required(true).value_parser([
                "bash",
                "zsh",
                "fish",
                "powershell",
                "elvish",
            ])),
    )
    .subcommand(
        Command::new("doctor").about("Report local configuration, catalog and build information"),
    )
}

pub type FriendlyGroup = (
    &'static str,
    &'static str,
    Vec<(&'static str, &'static str)>,
);
pub fn friendly_groups() -> Vec<FriendlyGroup> {
    vec![
        (
            "env",
            "Environment lifecycle and templates",
            vec![
                ("list", "environment/control/rest/getenvs"),
                ("info", "environment/control/rest/getenvinfo"),
                ("create", "environment/control/rest/createenvironment"),
                ("clone", "environment/control/rest/cloneenv"),
                ("start", "environment/control/rest/startenv"),
                ("stop", "environment/control/rest/stopenv"),
                ("delete", "environment/control/rest/deleteenv"),
                ("templates", "environment/control/rest/gettemplates"),
                ("regions", "environment/control/rest/getregions"),
            ],
        ),
        (
            "node",
            "Nodes and scaling",
            vec![
                ("list", "environment/control/rest/getenvinfo"),
                ("restart", "environment/control/rest/restartnodebyid"),
                ("scale", "environment/control/rest/setcloudletscountbyid"),
            ],
        ),
        (
            "logs",
            "Read a finite snapshot of platform logs",
            vec![
                ("list", "environment/control/rest/getlogslist"),
                ("read", "environment/control/rest/readlog"),
                ("watch", "environment/control/rest/readlog"),
            ],
        ),
        (
            "files",
            "Remote files",
            vec![
                ("list", "environment/file/rest/getlist"),
                ("read", "environment/file/rest/read"),
                ("write", "environment/file/rest/write"),
                ("upload", "environment/file/rest/upload"),
                ("delete", "environment/file/rest/delete"),
            ],
        ),
        (
            "vars",
            "Container variables (values use JSON file/stdin)",
            vec![
                ("list", "environment/control/rest/getcontainerenvvars"),
                ("set", "environment/control/rest/setcontainerenvvars"),
                ("delete", "environment/control/rest/removecontainerenvvars"),
            ],
        ),
        (
            "projects",
            "Cloud Management projects",
            vec![("list", "iaas/project/rest/get")],
        ),
        (
            "s3",
            "Object storage control plane",
            vec![
                ("list", "s3/bucket/rest/list"),
                ("create", "s3/bucket/rest/create"),
                ("delete", "s3/bucket/rest/delete"),
                ("objects", "s3/bucket/rest/listobjects"),
                ("create-account", "s3/account/rest/create"),
                ("keys", "s3/account/rest/getkeys"),
                ("generate-key", "s3/account/rest/generatekey"),
            ],
        ),
        (
            "billing",
            "Quotas and provider pricing (not a guaranteed cost estimate)",
            vec![
                ("quotas", "billing/account/rest/getquotas"),
                ("pricing", "billing/pricing/rest/getpricing"),
            ],
        ),
        (
            "operations",
            "Inspect asynchronous platform operations",
            vec![
                ("list", "environment/tracking/rest/getcurrentactions"),
                ("get", "environment/tracking/rest/getaction"),
            ],
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn command_tree_is_consistent() {
        let c = Catalog::embedded().unwrap();
        build(&c).debug_assert();
    }
    #[test]
    fn friendly_commands_use_real_methods() {
        let catalog = Catalog::embedded().unwrap();
        for group in friendly_groups() {
            for (_, path) in group.2 {
                assert!(catalog.find(path).is_some(), "{path}");
            }
        }
    }
    #[test]
    fn all_methods_reachable() {
        let c = Catalog::embedded().unwrap();
        let cmd = build(&c);
        for m in &c.methods {
            let parsed = cmd.clone().try_get_matches_from([
                "vap",
                "api",
                &m.namespace,
                &m.service,
                &m.name,
                "--help",
            ]);
            assert!(
                parsed.unwrap_err().kind() == clap::error::ErrorKind::DisplayHelp,
                "{}",
                m.path
            );
        }
    }
}
