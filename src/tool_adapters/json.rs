use crate::tool_runtime::IssueFinderToolInvocation;

pub fn parse_invocation(
    tool_name: String,
    arguments: &str,
    call_id: String,
    turn_id: Option<String>,
) -> Result<IssueFinderToolInvocation, String> {
    IssueFinderToolInvocation::from_json_arguments(tool_name, arguments, Some(call_id), turn_id)
}
