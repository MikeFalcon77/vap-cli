//! CLI boundary: turn parsed arguments and input bytes into a frontend-neutral request.
use crate::{
    catalog::{self, Catalog, Method},
    engine::{self, RunMode},
    error::{Error, Result},
    parameters::{self, ParameterInput, Targets},
    registry::{self, ArgKind, CommandSpec, ExecutionMode},
    request::{Authentication, PreparedRequest},
};
use clap::ArgMatches;
use serde_json::{Map, Value};

pub fn text<'a>(m: &'a ArgMatches, name: &str) -> Option<&'a str> {
    m.try_get_one::<String>(name)
        .ok()
        .flatten()
        .map(String::as_str)
}
pub fn flag(m: &ArgMatches, name: &str) -> bool {
    m.try_get_one::<bool>(name)
        .ok()
        .flatten()
        .copied()
        .unwrap_or(false)
}
pub struct Selection<'a> {
    pub method: Method,
    pub known: bool,
    pub spec: Option<&'static CommandSpec>,
    pub leaf: &'a ArgMatches,
    generated: bool,
}
pub struct Invocation {
    pub request: PreparedRequest,
    pub authentication: Authentication,
    pub spec: Option<&'static CommandSpec>,
    pub mode: RunMode,
}
impl<'a> Selection<'a> {
    pub fn resolve(root: &'a ArgMatches, catalog: &Catalog) -> Result<Self> {
        let (group, sub) = root
            .subcommand()
            .ok_or_else(|| Error::new("INVALID_INPUT", "Missing command"))?;
        if group == "api" {
            let (namespace, sub) = sub
                .subcommand()
                .ok_or_else(|| Error::new("INVALID_INPUT", "Missing API command"))?;
            if namespace == "call" {
                let path = text(sub, "path")
                    .unwrap()
                    .trim_matches('/')
                    .to_ascii_lowercase();
                catalog::validate_path(&path)?;
                let known = catalog.find(&path);
                let parts: Vec<_> = path.split('/').collect();
                let method = known.cloned().unwrap_or_else(|| Method {
                    path: path.clone(),
                    namespace: parts[0].into(),
                    service: parts[1].into(),
                    name: parts[3].into(),
                    public: false,
                    doc_url: None,
                    parameters: vec![],
                });
                return Ok(Self {
                    method,
                    known: known.is_some(),
                    spec: None,
                    leaf: sub,
                    generated: false,
                });
            }
            let (service, sub) = sub
                .subcommand()
                .ok_or_else(|| Error::new("INVALID_INPUT", "Missing API service"))?;
            let (name, leaf) = sub
                .subcommand()
                .ok_or_else(|| Error::new("INVALID_INPUT", "Missing API method"))?;
            let method = catalog
                .find(&format!("{namespace}/{service}/rest/{name}"))
                .ok_or_else(|| Error::new("UNKNOWN_METHOD", "Unknown API method"))?
                .clone();
            return Ok(Self {
                method,
                known: true,
                spec: None,
                leaf,
                generated: true,
            });
        }
        let (name, leaf) = if registry::COMMANDS
            .iter()
            .any(|s| s.group.is_empty() && s.name == group)
        {
            (group, sub)
        } else {
            sub.subcommand()
                .ok_or_else(|| Error::new("UNKNOWN_METHOD", "Unknown convenience command"))?
        };
        let spec = registry::COMMANDS
            .iter()
            .find(|s| s.name == name && (s.group == group || (s.group.is_empty() && name == group)))
            .ok_or_else(|| Error::new("UNKNOWN_METHOD", "Unknown convenience command"))?;
        Ok(Self {
            method: spec.method(catalog)?.clone(),
            known: true,
            spec: Some(spec),
            leaf,
            generated: false,
        })
    }
    pub fn prepare(
        self,
        root: &ArgMatches,
        saved: Targets,
        mut read: impl FnMut(&str) -> Result<String>,
    ) -> Result<Invocation> {
        // Enforce before credentials, file input and HTTP construction.
        PreparedRequest::new(&self.method.path, Map::new())?
            .enforce_read_only(flag(root, "read-only"))?;
        let values = if let Some(path) = text(root, "input") {
            serde_json::from_str::<Value>(&read(path)?)?
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
        let mut input = ParameterInput {
            values,
            saved,
            explicit: Targets {
                environment: text(root, "env").map(String::from),
                project: text(root, "project").map(String::from),
                node: text(root, "node").map(String::from),
            },
            ..Default::default()
        };
        if let Some(values) = root.get_many::<String>("param") {
            for value in values {
                let (name, value) = value
                    .split_once('=')
                    .ok_or_else(|| Error::new("INVALID_INPUT", "Use --param NAME=VALUE"))?;
                input.inline.push((name.into(), value.into()));
            }
        }
        if self.generated {
            for p in &self.method.parameters {
                if let Some(value) = text(self.leaf, &format!("api.{}", p.name)) {
                    input.inline.push((p.name.clone(), value.into()));
                }
            }
        }
        if let Some(spec) = self.spec {
            for arg in spec.args {
                let value = match arg.kind {
                    ArgKind::PositiveU32 => self
                        .leaf
                        .try_get_one::<u32>(arg.name)
                        .ok()
                        .flatten()
                        .map(|n| Value::from(*n)),
                    ArgKind::Text => text(self.leaf, arg.name).map(|v| Value::from(v.to_string())),
                    ArgKind::File => text(self.leaf, arg.name)
                        .map(&mut read)
                        .transpose()?
                        .map(Value::from),
                };
                if let Some(value) = value {
                    input.extra.insert(arg.name.into(), value);
                }
            }
        }
        let samples = input
            .extra
            .get("samples")
            .and_then(Value::as_u64)
            .unwrap_or(10) as u32;
        let params = parameters::prepare(&self.method, input, self.spec.and_then(|s| s.transform))?;
        let mode = if flag(root, "wait") {
            let (environment, target) = engine::wait_target(&self.method.path, &params)?;
            RunMode::Wait {
                environment,
                target,
            }
        } else if self.spec.is_some_and(|s| s.mode == ExecutionMode::Watch) {
            RunMode::Watch { samples }
        } else {
            RunMode::Single
        };
        Ok(Invocation {
            authentication: Authentication::for_method(&self.method, self.known),
            request: PreparedRequest::new(&self.method.path, params)?,
            spec: self.spec,
            mode,
        })
    }
}
