use crate::error::{Error, Result};
use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, io::Read};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Catalog {
    pub format_version: u32,
    pub platform_version: String,
    pub source: String,
    pub methods: Vec<Method>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Method {
    pub path: String,
    pub namespace: String,
    pub service: String,
    pub name: String,
    pub public: bool,
    pub doc_url: Option<String>,
    pub parameters: Vec<Parameter>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Parameter {
    pub name: String,
    pub kind: String,
    pub required: bool,
    #[serde(default)]
    pub aliases: Vec<String>,
}

impl Catalog {
    pub fn embedded() -> Result<Self> {
        let mut json = String::new();
        GzDecoder::new(&include_bytes!("../assets/catalog.json.gz")[..])
            .read_to_string(&mut json)?;
        Self::parse(&json)
    }

    pub fn parse(json: &str) -> Result<Self> {
        let catalog: Self = serde_json::from_str(json)?;
        if catalog.format_version != 1
            || catalog.methods.is_empty()
            || catalog.methods.len() > 20_000
        {
            return Err(Error::new(
                "INVALID_INPUT",
                "Unsupported or empty API catalog",
            ));
        }
        let mut seen = std::collections::BTreeSet::new();
        for method in &catalog.methods {
            validate_path(&method.path)?;
            if ["search", "describe", "schema", "call", "help"].contains(&method.namespace.as_str())
                || method.service == "help"
                || method.name == "help"
            {
                return Err(Error::new(
                    "INVALID_INPUT",
                    "Catalog method collides with a reserved discovery command",
                ));
            }
            let expected = format!(
                "{}/{}/rest/{}",
                method.namespace, method.service, method.name
            );
            if method.path != expected
                || method.path != method.path.to_ascii_lowercase()
                || !seen.insert(&method.path)
            {
                return Err(Error::new(
                    "INVALID_INPUT",
                    "Catalog has inconsistent or duplicate method paths",
                ));
            }
            let mut params = std::collections::BTreeSet::new();
            for param in &method.parameters {
                if param.name.is_empty()
                    || !param
                        .name
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_')
                    || !params.insert(&param.name)
                {
                    return Err(Error::new(
                        "INVALID_INPUT",
                        "Catalog contains an invalid parameter",
                    ));
                }
                if param.aliases.iter().any(|a| {
                    a.is_empty() || !a.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                }) {
                    return Err(Error::new(
                        "INVALID_INPUT",
                        "Catalog contains an invalid parameter alias",
                    ));
                }
            }
        }
        Ok(catalog)
    }

    pub fn find(&self, query: &str) -> Option<&Method> {
        let query = query.trim_matches('/').to_ascii_lowercase();
        self.methods.iter().find(|m| {
            m.path == query || format!("{}.{}.{}", m.namespace, m.service, m.name) == query
        })
    }

    pub fn stats(&self) -> Value {
        let mut domains = BTreeMap::<&str, usize>::new();
        for m in &self.methods {
            *domains.entry(&m.namespace).or_default() += 1;
        }
        serde_json::json!({"format_version":self.format_version,"platform_version":self.platform_version,"source":self.source,"methods":self.methods.len(),"public_methods":self.methods.iter().filter(|m|m.public).count(),"domains":domains,"coverage":"catalogued; live availability depends on platform version and permissions"})
    }
}

impl Method {
    // Only explicitly audited retrieval operations are retryable. Never infer safety from GET/POST.
    pub fn read_only(&self) -> bool {
        matches!(
            self.path.as_str(),
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
        )
    }
}

pub fn sensitive(name: &str) -> bool {
    let name: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect();
    [
        "password",
        "passwd",
        "passphrase",
        "token",
        "secret",
        "credential",
        "privatekey",
        "certkey",
        "accesskey",
        "apikey",
        "seckey",
        "acckey",
        "authorization",
        "authkey",
        "activationkey",
        "sshkey",
    ]
    .iter()
    .any(|part| name.contains(part))
        || name == "session"
        || name == "sessionid"
        || name == "auth"
        || name == "actionkey"
}

pub fn flag_name(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_uppercase() && !out.is_empty() {
            out.push('-');
        }
        out.push(c.to_ascii_lowercase());
    }
    out
}

pub fn requires_file(name: &str) -> bool {
    sensitive(name)
        || matches!(
            name.to_ascii_lowercase().as_str(),
            "key"
                | "vars"
                | "variables"
                | "data"
                | "body"
                | "commandlist"
                | "command"
                | "env"
                | "nodes"
                | "settings"
                | "manifest"
                | "value"
        )
}

pub fn validate_path(path: &str) -> Result<()> {
    let parts: Vec<_> = path.split('/').collect();
    if parts.len() != 4
        || parts[2] != "rest"
        || parts
            .iter()
            .any(|p| p.is_empty() || !p.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
    {
        return Err(Error::new(
            "INVALID_INPUT",
            "Use a relative REST path: namespace/service/rest/method",
        ));
    }
    Ok(())
}

pub fn normalize_provider(json: &str, version: &str, source: &str) -> Result<Catalog> {
    if let Ok(c) = Catalog::parse(json) {
        return Ok(c);
    }
    let data: Value = serde_json::from_str(json)?;
    if data.get("ok") == Some(&Value::Bool(true))
        && let Some(catalog) = data
            .get("data")
            .filter(|v| v.get("format_version").is_some())
    {
        return Catalog::parse(&serde_json::to_string(catalog)?);
    }
    if data
        .get("result")
        .and_then(Value::as_i64)
        .is_some_and(|v| v != 0)
    {
        return Err(Error::new(
            "API_ERROR",
            "Provider returned an error instead of a catalog",
        ));
    }
    let mut methods = BTreeMap::new();
    fn walk(n: &Value, out: &mut BTreeMap<String, Method>) -> Result<()> {
        if let Some(link) = n.get("link").and_then(Value::as_str) {
            let path = link
                .trim_start_matches('/')
                .strip_prefix("JElastic/")
                .unwrap_or(link.trim_start_matches('/'))
                .to_ascii_lowercase();
            validate_path(&path)?;
            let parts: Vec<_> = path.split('/').collect();
            let parameters = n
                .get("parameters")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .map(|p| {
                            Ok(Parameter {
                                name: p["name"]
                                    .as_str()
                                    .ok_or_else(|| {
                                        Error::new(
                                            "INVALID_INPUT",
                                            "Catalog parameter lacks a name",
                                        )
                                    })?
                                    .into(),
                                kind: p["type"].as_str().unwrap_or("String").into(),
                                required: !p["isOptional"].as_bool().unwrap_or(true),
                                aliases: p["altName"]
                                    .as_array()
                                    .map(|a| {
                                        a.iter()
                                            .filter_map(Value::as_str)
                                            .map(String::from)
                                            .collect()
                                    })
                                    .unwrap_or_default(),
                            })
                        })
                        .collect::<Result<Vec<_>>>()
                })
                .transpose()?
                .unwrap_or_default();
            out.insert(
                path.clone(),
                Method {
                    path: path.clone(),
                    namespace: parts[0].into(),
                    service: parts[1].into(),
                    name: parts[3].into(),
                    public: false,
                    doc_url: n["docLink"].as_str().map(String::from),
                    parameters,
                },
            );
        }
        for key in ["data", "children"] {
            if let Some(children) = n.get(key).and_then(Value::as_array) {
                for child in children {
                    walk(child, out)?;
                }
            }
        }
        Ok(())
    }
    walk(&data, &mut methods)?;
    let catalog = Catalog {
        format_version: 1,
        platform_version: version.into(),
        source: source.into(),
        methods: methods.into_values().collect(),
    };
    Catalog::parse(&serde_json::to_string(&catalog)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inventory() {
        let c = Catalog::embedded().unwrap();
        assert_eq!(c.methods.len(), 1376);
        assert_eq!(c.methods.iter().filter(|m| m.public).count(), 422);
        assert_eq!(c.methods.iter().filter(|m| m.namespace == "s3").count(), 18);
        assert!(c.find("environment.control.getenvs").unwrap().read_only());
        assert!(
            !c.find("environment.control.createenvironment")
                .unwrap()
                .read_only()
        );
    }
    #[test]
    fn endpoint_restrictions() {
        for path in [
            "https://evil.test",
            "../users/rest/a",
            "a/b/rest/c?session=x",
            "a/b/rest/c/../d",
        ] {
            assert!(validate_path(path).is_err());
        }
    }
}
