//! A validated request whose safety policy cannot be supplied by a caller or catalog.
use crate::{
    catalog::{self, Method},
    error::{Error, Result},
    registry::MethodPolicy,
};
use serde_json::{Map, Value};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Authentication {
    None,
    Optional,
    Required,
}
impl Authentication {
    pub fn for_method(method: &Method, known: bool) -> Self {
        if !known {
            return Self::Required;
        }
        match method.parameters.iter().find(|p| p.name == "session") {
            Some(p) if p.required => Self::Required,
            Some(_) => Self::Optional,
            None => Self::None,
        }
    }
}

pub struct PreparedRequest {
    path: String,
    params: Map<String, Value>,
    policy: MethodPolicy,
}
impl PreparedRequest {
    pub fn new(path: &str, params: Map<String, Value>) -> Result<Self> {
        catalog::validate_path(path)?;
        Ok(Self {
            path: path.into(),
            params,
            policy: MethodPolicy::for_path(path),
        })
    }
    pub fn path(&self) -> &str {
        &self.path
    }
    pub fn params(&self) -> &Map<String, Value> {
        &self.params
    }
    pub fn policy(&self) -> MethodPolicy {
        self.policy
    }
    pub fn enforce_read_only(&self, required: bool) -> Result<()> {
        if required && !self.policy.read_only() {
            return Err(Error::new(
                "PERMISSION_DENIED",
                "Operation is modifying or not audited as read-only",
            ));
        }
        Ok(())
    }
}
