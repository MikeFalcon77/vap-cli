//! Trusted command definitions and method policies. Never loaded from provider metadata.
use crate::{
    catalog::{Catalog, Method},
    error::{Error, Result},
};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MethodPolicy {
    ReadOnly,
    Modifying,
    Unaudited,
}
impl MethodPolicy {
    pub fn for_path(path: &str) -> Self {
        if matches!(
            path,
            "environment/control/rest/getenvs"
                | "environment/control/rest/getenvinfo"
                | "environment/control/rest/gettemplates"
                | "environment/control/rest/getregions"
                | "environment/control/rest/getallsumstat"
                | "environment/tracking/rest/getcurrentactions"
                | "environment/tracking/rest/getaction"
                | "environment/tracking/rest/getactions"
                | "environment/file/rest/getlist"
                | "environment/file/rest/read"
                | "environment/control/rest/getlogs"
                | "environment/control/rest/getlog"
                | "environment/control/rest/getloglist"
                | "environment/control/rest/getlogslist"
                | "environment/control/rest/readlog"
                | "environment/control/rest/getcontainernodetags"
                | "environment/control/rest/getenvvariables"
                | "environment/control/rest/getcontainervars"
                | "environment/control/rest/getcontainerenvvars"
                | "iaas/project/rest/get"
                | "s3/bucket/rest/list"
                | "s3/bucket/rest/listobjects"
                | "s3/account/rest/get"
                | "s3/account/rest/getkeys"
                | "billing/account/rest/getquotas"
                | "billing/pricing/rest/getpricing"
                | "users/authentication/rest/getuser"
                | "environment/control/rest/getnodegroups"
        ) {
            Self::ReadOnly
        } else if matches!(
            path,
            "environment/control/rest/createenvironment"
                | "environment/control/rest/cloneenv"
                | "environment/control/rest/startenv"
                | "environment/control/rest/stopenv"
                | "environment/control/rest/deleteenv"
        ) {
            Self::Modifying
        } else {
            Self::Unaudited
        }
    }
    pub fn read_only(self) -> bool {
        self == Self::ReadOnly
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecutionMode {
    Single,
    Watch,
}
#[derive(Clone, Copy)]
pub enum ArgKind {
    Text,
    File,
    PositiveU32,
}
#[derive(Clone, Copy)]
pub struct ExtraArg {
    pub name: &'static str,
    pub help: &'static str,
    pub kind: ArgKind,
    pub default: Option<&'static str>,
}
const NAME_ARGS: &[ExtraArg] = &[ExtraArg {
    name: "name",
    help: "New environment name",
    kind: ArgKind::Text,
    default: None,
}];
const CREATE_ARGS: &[ExtraArg] = &[
    NAME_ARGS[0],
    ExtraArg {
        name: "disk-gb",
        help: "Disk size in GB, applied to every node",
        kind: ArgKind::PositiveU32,
        default: None,
    },
];
const WATCH_ARGS: &[ExtraArg] = &[ExtraArg {
    name: "samples",
    help: "Finite number of log snapshots",
    kind: ArgKind::PositiveU32,
    default: Some("10"),
}];
const EXEC_ARGS: &[ExtraArg] = &[ExtraArg {
    name: "command-file",
    help: "Read the command string from a file; - reads stdin",
    kind: ArgKind::File,
    default: None,
}];

pub type View = fn(Value) -> Value;
pub struct CommandSpec {
    pub group: &'static str,
    pub group_help: &'static str,
    pub name: &'static str,
    pub path: &'static str,
    pub args: &'static [ExtraArg],
    pub transform: Option<crate::parameters::Transform>,
    pub view: Option<View>,
    pub mode: ExecutionMode,
}
impl CommandSpec {
    const fn new(
        group: &'static str,
        group_help: &'static str,
        name: &'static str,
        path: &'static str,
    ) -> Self {
        Self {
            group,
            group_help,
            name,
            path,
            args: &[],
            transform: None,
            view: None,
            mode: ExecutionMode::Single,
        }
    }
    pub fn method<'a>(&self, catalog: &'a Catalog) -> Result<&'a Method> {
        catalog.find(self.path).ok_or_else(|| {
            Error::new(
                "METHOD_UNAVAILABLE",
                "Convenience command is absent from the selected catalog",
            )
        })
    }
}
pub const COMMANDS: &[CommandSpec] = &[
    CommandSpec::new(
        "env",
        "Environment lifecycle and templates",
        "list",
        "environment/control/rest/getenvs",
    ),
    CommandSpec::new(
        "env",
        "Environment lifecycle and templates",
        "info",
        "environment/control/rest/getenvinfo",
    ),
    CommandSpec {
        args: CREATE_ARGS,
        transform: Some(crate::parameters::create_environment),
        ..CommandSpec::new(
            "env",
            "Environment lifecycle and templates",
            "create",
            "environment/control/rest/createenvironment",
        )
    },
    CommandSpec {
        args: NAME_ARGS,
        transform: Some(crate::parameters::clone_environment),
        ..CommandSpec::new(
            "env",
            "Environment lifecycle and templates",
            "clone",
            "environment/control/rest/cloneenv",
        )
    },
    CommandSpec::new(
        "env",
        "Environment lifecycle and templates",
        "start",
        "environment/control/rest/startenv",
    ),
    CommandSpec::new(
        "env",
        "Environment lifecycle and templates",
        "stop",
        "environment/control/rest/stopenv",
    ),
    CommandSpec::new(
        "env",
        "Environment lifecycle and templates",
        "delete",
        "environment/control/rest/deleteenv",
    ),
    CommandSpec::new(
        "env",
        "Environment lifecycle and templates",
        "templates",
        "environment/control/rest/gettemplates",
    ),
    CommandSpec::new(
        "env",
        "Environment lifecycle and templates",
        "regions",
        "environment/control/rest/getregions",
    ),
    CommandSpec {
        view: Some(nodes),
        ..CommandSpec::new(
            "node",
            "Nodes and scaling",
            "list",
            "environment/control/rest/getenvinfo",
        )
    },
    CommandSpec::new(
        "node",
        "Nodes and scaling",
        "restart",
        "environment/control/rest/restartnodebyid",
    ),
    CommandSpec::new(
        "node",
        "Nodes and scaling",
        "scale",
        "environment/control/rest/setcloudletscountbyid",
    ),
    CommandSpec::new(
        "logs",
        "Read a finite snapshot of platform logs",
        "list",
        "environment/control/rest/getlogslist",
    ),
    CommandSpec::new(
        "logs",
        "Read a finite snapshot of platform logs",
        "read",
        "environment/control/rest/readlog",
    ),
    CommandSpec {
        args: WATCH_ARGS,
        mode: ExecutionMode::Watch,
        ..CommandSpec::new(
            "logs",
            "Read a finite snapshot of platform logs",
            "watch",
            "environment/control/rest/readlog",
        )
    },
    CommandSpec::new(
        "files",
        "Remote files",
        "list",
        "environment/file/rest/getlist",
    ),
    CommandSpec::new(
        "files",
        "Remote files",
        "read",
        "environment/file/rest/read",
    ),
    CommandSpec::new(
        "files",
        "Remote files",
        "write",
        "environment/file/rest/write",
    ),
    CommandSpec::new(
        "files",
        "Remote files",
        "upload",
        "environment/file/rest/upload",
    ),
    CommandSpec::new(
        "files",
        "Remote files",
        "delete",
        "environment/file/rest/delete",
    ),
    CommandSpec::new(
        "vars",
        "Container variables (values use JSON file/stdin)",
        "list",
        "environment/control/rest/getcontainerenvvars",
    ),
    CommandSpec::new(
        "vars",
        "Container variables (values use JSON file/stdin)",
        "set",
        "environment/control/rest/setcontainerenvvars",
    ),
    CommandSpec::new(
        "vars",
        "Container variables (values use JSON file/stdin)",
        "delete",
        "environment/control/rest/removecontainerenvvars",
    ),
    CommandSpec::new(
        "projects",
        "Cloud Management projects",
        "list",
        "iaas/project/rest/get",
    ),
    CommandSpec::new(
        "s3",
        "Object storage control plane",
        "list",
        "s3/bucket/rest/list",
    ),
    CommandSpec::new(
        "s3",
        "Object storage control plane",
        "create",
        "s3/bucket/rest/create",
    ),
    CommandSpec::new(
        "s3",
        "Object storage control plane",
        "delete",
        "s3/bucket/rest/delete",
    ),
    CommandSpec::new(
        "s3",
        "Object storage control plane",
        "objects",
        "s3/bucket/rest/listobjects",
    ),
    CommandSpec::new(
        "s3",
        "Object storage control plane",
        "create-account",
        "s3/account/rest/create",
    ),
    CommandSpec::new(
        "s3",
        "Object storage control plane",
        "keys",
        "s3/account/rest/getkeys",
    ),
    CommandSpec::new(
        "s3",
        "Object storage control plane",
        "generate-key",
        "s3/account/rest/generatekey",
    ),
    CommandSpec::new(
        "billing",
        "Quotas and provider pricing (not a guaranteed cost estimate)",
        "quotas",
        "billing/account/rest/getquotas",
    ),
    CommandSpec::new(
        "billing",
        "Quotas and provider pricing (not a guaranteed cost estimate)",
        "pricing",
        "billing/pricing/rest/getpricing",
    ),
    CommandSpec::new(
        "operations",
        "Inspect asynchronous platform operations",
        "list",
        "environment/tracking/rest/getcurrentactions",
    ),
    CommandSpec::new(
        "operations",
        "Inspect asynchronous platform operations",
        "get",
        "environment/tracking/rest/getaction",
    ),
    CommandSpec {
        args: EXEC_ARGS,
        transform: Some(crate::parameters::exec_command),
        ..CommandSpec::new("", "", "exec", "environment/control/rest/execcmdbyid")
    },
];

pub fn environment_list(value: Value) -> Value {
    value
        .get("infos")
        .or_else(|| value.get("environments"))
        .cloned()
        .unwrap_or(value)
}
fn nodes(value: Value) -> Value {
    value.get("nodes").cloned().unwrap_or(value)
}
pub fn project(path: &str, spec: Option<&CommandSpec>, value: Value) -> Value {
    if let Some(view) = spec.and_then(|s| s.view) {
        view(value)
    } else if path == "environment/control/rest/getenvs" {
        environment_list(value)
    } else {
        value
    }
}

// These legacy tracking endpoints accept an environment name in their appid field.
pub fn environment_appid(path: &str) -> bool {
    matches!(
        path,
        "environment/tracking/rest/getaction"
            | "environment/tracking/rest/getactions"
            | "environment/tracking/rest/getallservicename"
            | "environment/tracking/rest/getcurrentactions"
            | "environment/tracking/rest/getenvactions"
            | "environment/tracking/rest/getserverutctime"
            | "environment/tracking/rest/getuidactions"
            | "environment/tracking/rest/getuidactionsadmin"
            | "environment/tracking/rest/searchactions"
            | "environment/tracking/rest/storeauditadminactions"
            | "environment/tracking/rest/storeuseractions"
    )
}
