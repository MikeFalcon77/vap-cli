use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct Error {
    pub code: &'static str,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_code: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            api_code: None,
            hint: None,
        }
    }
    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
    pub fn exit_code(&self) -> i32 {
        match self.code {
            "INVALID_INPUT" | "UNKNOWN_METHOD" | "AMBIGUOUS_TARGET" => 2,
            "NOT_AUTHENTICATED" | "SESSION_EXPIRED" => 3,
            "PERMISSION_DENIED" | "CONFIRMATION_REQUIRED" => 4,
            "NETWORK_ERROR" => 5,
            "TIMEOUT" | "OUTCOME_UNKNOWN" => 6,
            "INTERRUPTED" => 130,
            _ => 1,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(_: std::io::Error) -> Self {
        Self::new(
            "IO_ERROR",
            "Cannot read or write the requested file or stream",
        )
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Self::new(
            "INVALID_INPUT",
            format!("Invalid JSON at line {}, column {}", e.line(), e.column()),
        )
    }
}
