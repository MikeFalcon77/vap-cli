//! Pure parameter assembly shared by every frontend.
use crate::{
    catalog::{self, Method},
    error::{Error, Result},
    output,
};
use serde_json::{Map, Value, json};
#[derive(Clone, Default)]
pub struct Targets {
    pub environment: Option<String>,
    pub project: Option<String>,
    pub node: Option<String>,
}
impl Targets {
    pub fn get(&self, name: &str) -> Option<&str> {
        match name {
            "env" => self.environment.as_deref(),
            "project" => self.project.as_deref(),
            _ => self.node.as_deref(),
        }
    }
}
#[derive(Default)]
pub struct ParameterInput {
    pub values: Map<String, Value>,
    pub inline: Vec<(String, String)>,
    pub explicit: Targets,
    pub saved: Targets,
    pub extra: Map<String, Value>,
}
pub type Transform = fn(&mut Map<String, Value>, &Map<String, Value>) -> Result<()>;
pub fn parse_parameter(value: &str, kind: &str) -> Result<Value> {
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
pub fn inline_allowed(name: &str, value: &Value) -> Result<()> {
    if catalog::requires_file(name) || !output::secret_values(value, true).is_empty() {
        return Err(Error::new(
            "INVALID_INPUT",
            "Secret or opaque payload parameters cannot be supplied in command arguments",
        )
        .hint("Use --input file.json or --input -"));
    }
    Ok(())
}

pub fn prepare(
    method: &Method,
    input: ParameterInput,
    transform: Option<Transform>,
) -> Result<Map<String, Value>> {
    let mut params = input.values;
    for (name, value) in &input.inline {
        let p = method
            .parameters
            .iter()
            .find(|p| p.name == *name || p.aliases.contains(name));
        let parsed = parse_parameter(value, p.map(|p| p.kind.as_str()).unwrap_or("String"))?;
        inline_allowed(p.map(|p| p.name.as_str()).unwrap_or(name), &parsed)?;
        params.insert(name.clone(), parsed);
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
    if params.keys().any(|k| k.eq_ignore_ascii_case("session")) {
        return Err(Error::new(
            "INVALID_INPUT",
            "Session is injected from the profile; use VAP_TOKEN to override authentication",
        ));
    }
    for (target, saved) in [
        ("env", input.saved.environment.as_deref()),
        ("project", input.saved.project.as_deref()),
        ("node", input.saved.node.as_deref()),
    ] {
        let explicit = input.explicit.get(target);
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
                            && (crate::registry::environment_appid(&method.path)
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
    if let Some(transform) = transform {
        transform(&mut params, &input.extra)?;
    }
    if method.path == "environment/control/rest/readlog" {
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
pub fn create_environment(
    params: &mut Map<String, Value>,
    extra: &Map<String, Value>,
) -> Result<()> {
    if let Some(name) = extra.get("name") {
        params
            .entry("env")
            .or_insert(json!({}))
            .as_object_mut()
            .ok_or_else(|| Error::new("INVALID_INPUT", "CreateEnvironment env must be an object"))?
            .insert("shortdomain".into(), name.clone());
    }
    if let Some(disk) = extra.get("disk-gb") {
        for node in params
            .get_mut("nodes")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| {
                Error::new(
                    "INVALID_INPUT",
                    "--disk-gb requires a nodes array in --input",
                )
            })?
        {
            node.as_object_mut()
                .ok_or_else(|| Error::new("INVALID_INPUT", "Each node must be an object"))?
                .insert("diskLimit".into(), disk.clone());
        }
    }
    Ok(())
}
pub fn clone_environment(
    params: &mut Map<String, Value>,
    extra: &Map<String, Value>,
) -> Result<()> {
    if let Some(name) = extra.get("name") {
        params.insert("dstEnvName".into(), name.clone());
    }
    Ok(())
}
pub fn exec_command(params: &mut Map<String, Value>, extra: &Map<String, Value>) -> Result<()> {
    if let Some(command) = extra.get("command-file") {
        params.insert("commandList".into(), json!([{"command":command}]));
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inline_nested_secrets_rejected() {
        assert!(inline_allowed("options", &json!({"password":"secret"})).is_err());
        assert!(inline_allowed("commandList", &json!("echo hidden")).is_err());
    }
}
