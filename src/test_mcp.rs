
use rust_mcp_sdk::schema::GetPromptResult;
fn check_mcp(r: &mut GetPromptResult) {
    let t: () = &mut r.messages[0].content;
}

