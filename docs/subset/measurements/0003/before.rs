pub enum Inbound {
    Event(ClaudeEvent),
    ControlResponse { request_id: String, outcome: Result<Value, String> },
    UnsupportedControlRequest { request_id: String, subtype: String },
}
pub fn parse_line(line: &str) -> serde_json::Result<Inbound> { todo() }
pub struct UnsupportedVersion {
    pub found: String,
    pub supported: String,
}
pub fn check_version(initialize: Value) -> Result<(), UnsupportedVersion> { todo() }
pub enum ControlCall { Initialize, Interrupt, SetPermissionMode(PermissionMode) }
impl ControlCall {
    pub fn subtype(&self) -> &str { todo() }
}
pub fn encode(msg: Outbound) -> serde_json::Result<String> { todo() }
