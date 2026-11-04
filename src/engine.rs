//! Operation execution: no Clap, prompts, configuration reads or stdout.
use crate::{
    error::{Error, Result},
    request::PreparedRequest,
    transport,
};
use serde_json::{Map, Value, json};
use std::time::Duration;
use tokio::time::Instant;

pub enum RunMode {
    Single,
    Wait {
        environment: String,
        target: EnvTarget,
    },
    Watch {
        samples: u32,
    },
}
pub struct Event {
    pub data: Value,
    pub meta: Value,
}
pub enum ExecutionResult {
    Response(Value),
    StreamComplete,
}
pub struct Engine<'a> {
    pub client: &'a transport::Client,
    pub credential: Option<&'a str>,
    pub timeout: Duration,
    pub interval: Duration,
}
impl Engine<'_> {
    pub async fn run(
        &self,
        request: &PreparedRequest,
        mode: RunMode,
        mut initial: impl FnMut(&Value) -> Result<()>,
        mut emit: impl FnMut(Event) -> Result<()>,
    ) -> Result<ExecutionResult> {
        let deadline = Instant::now() + self.timeout;
        let waiting = matches!(mode, RunMode::Wait { .. });
        let timeout = || {
            Error::new(
                if waiting || !request.policy().read_only() {
                    "OUTCOME_UNKNOWN"
                } else {
                    "TIMEOUT"
                },
                "Operation deadline reached; inspect resource state before retrying",
            )
        };
        let future = async {
            let raw = self.client.call(request, self.credential, deadline).await?;
            initial(&raw)?;
            let response = transport::payload(raw);
            match mode {
                RunMode::Single => Ok(ExecutionResult::Response(response)),
                RunMode::Wait {
                    environment,
                    target,
                } => {
                    let poll =
                        PreparedRequest::new("environment/control/rest/getenvs", Map::new())?;
                    loop {
                        let state = transport::payload(
                            self.client.call(&poll, self.credential, deadline).await?,
                        );
                        if env_finished(&state, &environment, target)? {
                            return Ok(ExecutionResult::Response(
                                json!({"environment":environment,"completed":true,"state":state}),
                            ));
                        }
                        tokio::time::sleep(self.interval).await;
                    }
                }
                RunMode::Watch { samples } => {
                    let mut previous = None;
                    for index in 0..samples {
                        let data = if index == 0 {
                            response.clone()
                        } else {
                            tokio::time::sleep(self.interval).await;
                            transport::payload(
                                self.client.call(request, self.credential, deadline).await?,
                            )
                        };
                        if previous.as_ref() != Some(&data) {
                            emit(Event {
                                data: data.clone(),
                                meta: json!({"event":"log_snapshot","sample":index+1,"samples":samples}),
                            })?;
                            previous = Some(data);
                        }
                    }
                    Ok(ExecutionResult::StreamComplete)
                }
            }
        };
        tokio::time::timeout_at(deadline, future)
            .await
            .map_err(|_| timeout())?
            .map_err(|e: Error| {
                if waiting && e.code == "TIMEOUT" {
                    timeout()
                } else {
                    e
                }
            })
    }
}
#[derive(Clone, Copy)]
pub enum EnvTarget {
    Running,
    Stopped,
    Absent,
}
pub fn wait_target(path: &str, params: &Map<String, Value>) -> Result<(String, EnvTarget)> {
    let (name, target) = match path {
        "environment/control/rest/createenvironment" => (
            params
                .get("env")
                .and_then(|e| e.get("shortdomain"))
                .and_then(Value::as_str),
            EnvTarget::Running,
        ),
        "environment/control/rest/cloneenv" => (
            params.get("dstEnvName").and_then(Value::as_str),
            EnvTarget::Running,
        ),
        "environment/control/rest/startenv" => (
            params.get("envName").and_then(Value::as_str),
            EnvTarget::Running,
        ),
        "environment/control/rest/stopenv" => (
            params.get("envName").and_then(Value::as_str),
            EnvTarget::Stopped,
        ),
        "environment/control/rest/deleteenv" => (
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
}
