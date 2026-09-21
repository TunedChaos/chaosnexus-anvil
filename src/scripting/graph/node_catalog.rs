// chaosnexus-anvil/src/scripting/graph/node_catalog.rs
//
// Stable dispatch keys for Vhai logic nodes (mirrors Forge node_catalog.ts).

/// Standard execution pin ids.
pub const EXEC_IN: &str = "exec_in";
pub const EXEC_OUT: &str = "exec_out";
pub const EXEC_TRUE: &str = "true";
pub const EXEC_FALSE: &str = "false";
pub const EXEC_BODY: &str = "body";
pub const EXEC_COMPLETED: &str = "completed";
pub const EXEC_CATCH: &str = "catch";
pub const RETURN_HANDLE: &str = "return";

/// Node kind strings (must match Forge `CanvasNodeKind`).
pub const KIND_EVENT: &str = "event";
pub const KIND_FUNCTION: &str = "function";
pub const KIND_BRANCH: &str = "branch";
pub const KIND_ITERATOR: &str = "iterator";
pub const KIND_SEQUENCE: &str = "sequence";
pub const KIND_WHILE: &str = "while";
pub const KIND_LOOP: &str = "loop";
pub const KIND_DO_WHILE: &str = "do-while";
pub const KIND_FOR_EACH: &str = "for-each";
pub const KIND_BREAK: &str = "break";
pub const KIND_CONTINUE: &str = "continue";
pub const KIND_RETURN: &str = "return";
pub const KIND_TRY_CATCH: &str = "try-catch";
pub const KIND_SWITCH: &str = "switch";
pub const KIND_LITERAL: &str = "literal";
pub const KIND_GET_VARIABLE: &str = "get-variable";
pub const KIND_SET_VARIABLE: &str = "set-variable";
pub const KIND_OPERATOR: &str = "operator";
pub const KIND_MAKE_ARRAY: &str = "make-array";
pub const KIND_MAKE_MAP: &str = "make-map";
pub const KIND_INDEX: &str = "index";
pub const KIND_MEMBER_GET: &str = "member-get";
pub const KIND_SCRIPT: &str = "script";
pub const KIND_EXPRESSION: &str = "expression";
pub const KIND_ML_TRAIN: &str = "ml-train";
pub const KIND_ML_PREDICT: &str = "ml-predict";
pub const KIND_SCI_MATH: &str = "sci-math";
pub const KIND_COMMENT: &str = "comment";

/// Returns true when `kind` is handled by the exec VM natively (not a Rhai fn call).
pub fn is_exec_native_kind(kind: &str) -> bool {
    matches!(
        kind,
        KIND_EVENT
            | KIND_SEQUENCE
            | KIND_BRANCH
            | KIND_WHILE
            | KIND_LOOP
            | KIND_DO_WHILE
            | KIND_FOR_EACH
            | KIND_BREAK
            | KIND_CONTINUE
            | KIND_RETURN
            | KIND_TRY_CATCH
            | KIND_SWITCH
            | KIND_LITERAL
            | KIND_GET_VARIABLE
            | KIND_SET_VARIABLE
            | KIND_OPERATOR
            | KIND_MAKE_ARRAY
            | KIND_MAKE_MAP
            | KIND_INDEX
            | KIND_MEMBER_GET
            | KIND_SCRIPT
            | KIND_EXPRESSION
            | KIND_COMMENT
            | KIND_ITERATOR
    )
}

/// Rhai expression template for operator micro-AST eval (`a`, `b` placeholders).
pub fn operator_expression(operator_id: &str) -> Option<&'static str> {
    match operator_id {
        "add" => Some("a + b"),
        "sub" => Some("a - b"),
        "mul" => Some("a * b"),
        "div" => Some("a / b"),
        "mod" => Some("a % b"),
        "pow" => Some("a ** b"),
        "eq" => Some("a == b"),
        "ne" => Some("a != b"),
        "lt" => Some("a < b"),
        "le" => Some("a <= b"),
        "gt" => Some("a > b"),
        "ge" => Some("a >= b"),
        "and" => Some("a && b"),
        "or" => Some("a || b"),
        "not" => Some("!a"),
        "neg" => Some("-a"),
        "bit_and" => Some("a & b"),
        "bit_or" => Some("a | b"),
        "bit_xor" => Some("a ^ b"),
        "shl" => Some("a << b"),
        "shr" => Some("a >> b"),
        "in" => Some("a in b"),
        _ => None,
    }
}
