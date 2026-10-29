use serde::Serialize;

/// Bound report text without changing runtime values or exception messages.
pub(crate) fn bounded_diagnostic(message: impl Into<String>) -> String {
    let mut message = message.into();
    if message.len() > 8192 {
        let mut end = 8192;
        while !message.is_char_boundary(end) {
            end -= 1;
        }
        message.truncate(end);
        message.push_str(" [diagnostic truncated]");
    }
    message
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
    pub code: String,
    pub severity: String,
    pub message: String,
    pub path: String,
    pub line: usize,
    pub column: usize,
    pub end_line: usize,
    pub end_column: usize,
}
impl Diagnostic {
    pub fn plain(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            severity: "error".into(),
            message: bounded_diagnostic(message),
            path: String::new(),
            line: 1,
            column: 1,
            end_line: 1,
            end_column: 2,
        }
    }
}
#[derive(Clone, Debug, Serialize)]
pub struct FileInfo {
    pub path: String,
    pub selected: bool,
    pub classes: Vec<String>,
}
#[derive(Clone, Copy, Debug, Default, Serialize, PartialEq, Eq)]
pub struct AssertionCounts {
    pub executed: usize,
    pub passed: usize,
    pub failed: usize,
}
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct InputDigest {
    pub path: String,
    pub sha256: String,
}
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InputDirectoryDigest {
    pub path: String,
    pub sha256: String,
    pub include_ignored: bool,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestResult {
    pub id: String,
    pub path: String,
    pub line: usize,
    pub status: String,
    pub message: String,
    pub duration_ms: f64,
    pub assertions: AssertionCounts,
    pub setup_assertions: AssertionCounts,
}
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub files: usize,
    pub selected_files: usize,
    pub tests: usize,
    pub passed: usize,
    pub failed: usize,
    pub blocked: usize,
    pub errored: usize,
    pub duration_ms: f64,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub schema_version: u32,
    pub engine_version: String,
    pub command: String,
    pub status: String,
    pub assurance: String,
    pub exit_code: i32,
    pub project_root: String,
    pub files: Vec<FileInfo>,
    pub inputs: Vec<InputDigest>,
    pub input_directories: Vec<InputDirectoryDigest>,
    pub input_fingerprint: String,
    pub diagnostics: Vec<Diagnostic>,
    pub tests: Vec<TestResult>,
    pub summary: Summary,
    pub limitations: Vec<String>,
}
impl Report {
    pub fn new(command: &str) -> Self {
        Self{schema_version:2,engine_version:env!("CARGO_PKG_VERSION").into(),command:command.into(),status:"errored".into(),assurance:if command=="check" {"syntax-and-declarations"}else{"experimental-local-execution"}.into(),exit_code:3,project_root:String::new(),files:vec![],inputs:vec![],input_directories:vec![],input_fingerprint:crate::inputs::fingerprint(&[], &[]),diagnostics:vec![],tests:vec![],summary:Summary::default(),limitations:vec!["Experimental preview. No Salesforce oracle qualification; successful execution returns exit 4.".into(),"Check validates syntax, declarations and a bounded semantic subset, not complete Apex typing, security, metadata semantics or deployment readiness.".into(),"Runtime supports an explicitly bounded subset. Unimplemented reached behavior blocks instead of returning defaults.".into()]}
    }
}
