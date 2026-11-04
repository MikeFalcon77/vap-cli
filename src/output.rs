use crate::{
    catalog::sensitive,
    error::{Error, Result},
};
use serde_json::{Value, json};
use std::io::{self, IsTerminal, Write};

pub fn redact(value: &mut Value, secrets: &[String], private_keys: bool) {
    match value {
        Value::Object(map) => {
            for (name, value) in map {
                if (sensitive(name) && !value.is_boolean())
                    || (private_keys && name == "key" && value.is_string())
                    || matches!(
                        name.to_ascii_lowercase().as_str(),
                        "variables"
                            | "vars"
                            | "envvars"
                            | "envvariables"
                            | "commandlist"
                            | "command"
                    )
                {
                    *value = json!("[REDACTED]");
                } else {
                    redact(value, secrets, private_keys);
                }
            }
        }
        Value::Array(values) => {
            for v in values {
                redact(v, secrets, private_keys);
            }
        }
        Value::String(text) => {
            for secret in secrets.iter().filter(|s| !s.is_empty()) {
                *text = text.replace(secret, "[REDACTED]");
            }
        }
        _ => {}
    }
}

pub fn secret_values(value: &Value, private_keys: bool) -> Vec<String> {
    fn walk(value: &Value, sensitive_parent: bool, private_keys: bool, out: &mut Vec<String>) {
        match value {
            Value::Object(map) => {
                for (k, v) in map {
                    walk(
                        v,
                        sensitive_parent
                            || sensitive(k)
                            || (private_keys && k == "key")
                            || matches!(
                                k.as_str(),
                                "value" | "vars" | "variables" | "commandList" | "command"
                            ),
                        private_keys,
                        out,
                    );
                }
            }
            Value::Array(values) => {
                for v in values {
                    walk(v, sensitive_parent, private_keys, out);
                }
            }
            Value::String(s) if sensitive_parent => out.push(s.clone()),
            _ => {}
        }
    }
    let mut out = vec![];
    walk(value, false, private_keys, &mut out);
    out
}

pub fn project(mut data: Value, fields: &[String], limit: Option<usize>) -> (Value, Value) {
    let mut meta = json!({"truncated":false});
    if let Some(groups) = data.get_mut("bucketsList").and_then(Value::as_array_mut) {
        let total: usize = groups
            .iter()
            .filter_map(|g| g.get("buckets").and_then(Value::as_array))
            .map(Vec::len)
            .sum();
        let mut remaining = limit.unwrap_or(usize::MAX);
        let mut returned = 0;
        for group in groups {
            if let Some(buckets) = group.get_mut("buckets").and_then(Value::as_array_mut) {
                buckets.truncate(remaining);
                remaining = remaining.saturating_sub(buckets.len());
                returned += buckets.len();
                if !fields.is_empty() {
                    for bucket in buckets {
                        *bucket = select_fields(bucket, fields);
                    }
                }
            }
        }
        return (
            data,
            json!({"truncated":returned<total,"total":total,"returned":returned,"collection":"bucketsList[].buckets"}),
        );
    }
    // Arrays are projected at the root or at the first standard collection key.
    let array = if data.is_array() {
        data.as_array_mut()
    } else {
        data.as_object_mut().and_then(|m| {
            let key = [
                "data",
                "array",
                "responses",
                "objects",
                "environments",
                "nodes",
                "actions",
                "projects",
                "buckets",
                "list",
                "items",
                "methods",
                "infos",
                "regions",
                "templates",
            ]
            .iter()
            .find(|k| m.get(**k).is_some_and(Value::is_array))
            .copied();
            key.and_then(|k| m.get_mut(k)).and_then(Value::as_array_mut)
        })
    };
    if let Some(array) = array {
        let total = array.len();
        if let Some(limit) = limit {
            array.truncate(limit);
        }
        if !fields.is_empty() {
            for value in array.iter_mut() {
                *value = select_fields(value, fields);
            }
        }
        meta = json!({"truncated":array.len()<total,"total":total,"returned":array.len()});
    } else if !fields.is_empty() {
        data = select_fields(&data, fields);
    }
    (data, meta)
}

fn select_fields(value: &Value, fields: &[String]) -> Value {
    let mut map = serde_json::Map::new();
    for field in fields {
        let mut v = value;
        for part in field.split('.') {
            v = v.get(part).unwrap_or(&Value::Null);
        }
        map.insert(field.clone(), v.clone());
    }
    Value::Object(map)
}

pub fn success(data: Value, meta: Value, json_mode: bool) -> Result<()> {
    let mut out = io::stdout().lock();
    if json_mode {
        serde_json::to_writer(
            &mut out,
            &json!({"schema_version":1,"ok":true,"data":data,"error":null,"meta":meta}),
        )?;
        writeln!(out)?;
    } else {
        if let Some(groups) = data.get("bucketsList").and_then(Value::as_array) {
            let rows:Vec<Value>=groups.iter().flat_map(|g|g.get("buckets").and_then(Value::as_array).into_iter().flatten().map(|b|json!({"bucket":b["name"],"region":g["hostGroup"]["hostGroupDisplayName"],"bytes":b["size"],"created":b["creationDate"]}))).collect();
            table(&mut out, &rows)?;
        } else if let Some(items) = data.as_array().or_else(|| {
            ["array", "responses", "objects", "nodes", "items"]
                .iter()
                .find_map(|key| data.get(key).and_then(Value::as_array))
        }) {
            table(&mut out, items)?;
        } else {
            writeln!(out, "{}", serde_json::to_string_pretty(&data)?)?;
        }
        if meta["truncated"] == true {
            writeln!(
                io::stderr(),
                "Output truncated: {} of {} items",
                meta["returned"],
                meta["total"]
            )?;
        }
    }
    Ok(())
}

fn table(out: &mut impl Write, items: &[Value]) -> Result<()> {
    if items.is_empty() {
        writeln!(out, "No items")?;
        return Ok(());
    }
    let summaries:Vec<Value>=items.iter().map(|item|{
        if let Some(env)=item.get("env").filter(|v|v.is_object()) {
            let status=env["status"].as_i64().map(|s|match s{1=>"running".into(),2=>"stopped".into(),6=>"provisioning".into(),_=>s.to_string()}).unwrap_or_else(||"unknown".into());
            json!({"environment":env.get("envName").unwrap_or(&Value::Null),"status":status,"nodes":item.get("nodes").and_then(Value::as_array).map(Vec::len),"region":env.get("region").unwrap_or(&Value::Null)})
        }else if let Some(quota)=item.get("quota").filter(|q|q.is_object()) {
            json!({"quota":quota["name"],"value":item["value"],"description":quota["description"]})
        }else{item.clone()}
    }).collect();
    let items = &summaries;
    let columns: Vec<_> = items[0]
        .as_object()
        .map(|m| {
            m.keys()
                .filter(|k| {
                    items[0][*k].is_string()
                        || items[0][*k].is_number()
                        || items[0][*k].is_boolean()
                })
                .take(5)
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    if columns.is_empty() {
        writeln!(out, "{}", serde_json::to_string_pretty(items)?)?;
        return Ok(());
    }
    let width = std::env::var("COLUMNS")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(100)
        .clamp(30, 240)
        / columns.len();
    let colored = io::stdout().is_terminal()
        && std::env::var_os("NO_COLOR").is_none()
        && std::env::var("TERM").as_deref() != Ok("dumb");
    if colored {
        write!(out, "\x1b[1;36m")?;
    }
    for c in &columns {
        write!(out, "{:<width$}", short(c, width))?;
    }
    if colored {
        write!(out, "\x1b[0m")?;
    }
    writeln!(out)?;
    for item in items {
        for c in &columns {
            let text = item[c]
                .as_str()
                .map(String::from)
                .unwrap_or_else(|| item[c].to_string());
            write!(out, "{:<width$}", short(&text, width))?;
        }
        writeln!(out)?;
    }
    Ok(())
}

fn short(s: &str, width: usize) -> String {
    let cleaned: String = s.chars().filter(|c| !c.is_control()).collect();
    if cleaned.chars().count() < width {
        cleaned
    } else {
        cleaned
            .chars()
            .take(width.saturating_sub(2))
            .chain(['…', ' '])
            .collect()
    }
}

pub fn failure(error: &Error, json_mode: bool) {
    if json_mode {
        let _ = writeln!(
            io::stdout(),
            "{}",
            json!({"schema_version":1,"ok":false,"data":null,"error":error,"meta":{}})
        );
    } else {
        let _ = writeln!(io::stderr(), "{}: {}", error.code, error.message);
        if let Some(hint) = &error.hint {
            let _ = writeln!(io::stderr(), "Hint: {hint}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn provider_collections_and_boolean_capabilities() {
        let (v, m) = project(
            json!({"array":[{"id":1},{"id":2}],"result":0}),
            &["id".into()],
            Some(1),
        );
        assert_eq!(v["array"].as_array().unwrap().len(), 1);
        assert_eq!(m["total"], 2);
        let (v, m) = project(
            json!({"responses":[{"name":"a"},{"name":"b"}],"result":0}),
            &[],
            Some(1),
        );
        assert_eq!(v["responses"].as_array().unwrap().len(), 1);
        assert_eq!(m["truncated"], true);
        let (v, m) = project(
            json!({"bucketsList":[{"buckets":[{"name":"a"},{"name":"b"}]},{"buckets":[{"name":"c"}]}],"result":0}),
            &["name".into()],
            Some(1),
        );
        assert_eq!(v["bucketsList"][0]["buckets"], json!([{"name":"a"}]));
        assert_eq!(v["bucketsList"][1]["buckets"], json!([]));
        assert_eq!(m["total"], 3);
        assert_eq!(m["returned"], 1);
        let mut flags = json!({"isResetPassword":true,"isToken":false,"password":"secret"});
        redact(&mut flags, &[], true);
        assert_eq!(flags["isResetPassword"], true);
        assert_eq!(flags["isToken"], false);
        assert_eq!(flags["password"], "[REDACTED]");
    }
    #[test]
    fn certificate_keys_and_passphrases_are_secrets_but_s3_object_names_are_not() {
        for name in [
            "cert_key",
            "private_key",
            "api_key",
            "authKey",
            "passphrase",
            "sshKey",
        ] {
            assert!(sensitive(name), "{name}");
        }
        let mut s3 = json!({"key":"folder/object"});
        redact(&mut s3, &[], false);
        assert_eq!(s3["key"], "folder/object");
        let mut ssl = json!({"key":"PRIVATE_KEY_SENTINEL"});
        redact(&mut ssl, &[], true);
        assert_eq!(ssl["key"], "[REDACTED]");
    }
    #[test]
    fn redacts_nested_and_echoed_secrets() {
        let mut v = json!({"key":{"accKey":"abc","secKey":"def"},"echo":"token xyz","vars":{"DB_PASSWORD":"123"}});
        redact(&mut v, &["xyz".into()], true);
        let s = v.to_string();
        for secret in ["abc", "def", "xyz", "123"] {
            assert!(!s.contains(secret));
        }
    }
    #[test]
    fn truncation_is_explicit() {
        let (d, m) = project(
            json!({"items":[{"id":1},{"id":2}]}),
            &["id".into()],
            Some(1),
        );
        assert_eq!(d["items"].as_array().unwrap().len(), 1);
        assert_eq!(m["truncated"], true);
        assert_eq!(m["total"], 2);
    }
}
