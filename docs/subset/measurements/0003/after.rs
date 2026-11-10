pub(crate) enum Inbound {
    Event(ClaudeEvent),
    ControlResponse { request_id: String, outcome: Result<Value, String> },
    UnsupportedControlRequest { request_id: String, subtype: String },
}
pub(crate) fn parse_line(line: &str) -> serde_json::Result<Inbound> { todo() }
pub(crate) struct UnsupportedVersion {
    pub(crate) found: String,
    pub(crate) supported: String,
}
pub(crate) fn check_version(initialize: Value) -> Result<(), UnsupportedVersion> { todo() }
pub(crate) enum ControlCall { Initialize, Interrupt, SetPermissionMode(PermissionMode) }
impl ControlCall {
    pub(crate) fn subtype(&self) -> &str { todo() }
}
pub(crate) fn encode(msg: Outbound) -> serde_json::Result<String> { todo() }
