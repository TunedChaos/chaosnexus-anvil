// chaosnexus-anvil/src/scripting/native_api/gates.rs
//
// Shared capability and identity checks for native API bindings.

use crate::scripting::capabilities::Capability;
use crate::scripting::models::NativeContext;
use crate::scripting::plugin_context::{
    capabilities_for, current_plugin_name, require_capability, verify_plugin_identity,
};
use rhai::EvalAltResult;

/// Type alias for gate evaluation results.
pub type GateResult = Result<(), Box<EvalAltResult>>;

/// Requires a specific capability to proceed.
pub fn require_cap(ctx: &NativeContext, cap: Capability) -> GateResult {
    require_capability(&ctx.plugin_capabilities, cap).map_err(|e| e.into())
}

/// Verifies that the caller plugin has the required cross-plugin capabilities.
pub fn verify_caller_plugin(
    ctx: &NativeContext,
    claimed_plugin: &str,
    cross_cap: Capability,
) -> GateResult {
    let caps = capabilities_for(&ctx.plugin_capabilities, claimed_plugin);
    if let Some(current) = crate::scripting::plugin_context::current_plugin() {
        let caller_caps = capabilities_for(&ctx.plugin_capabilities, &current);
        verify_plugin_identity(claimed_plugin, &caller_caps, cross_cap)
            .map_err(|e: String| -> Box<EvalAltResult> { e.into() })?;
        let _ = caps;
    }
    Ok(())
}

/// Verifies the caller plugin identity against cross-plugin capabilities and returns the current plugin name.
pub fn verify_current_plugin(
    ctx: &NativeContext,
    cross_cap: Capability,
) -> Result<String, Box<EvalAltResult>> {
    let current_plugin = current_plugin_name();
    verify_caller_plugin(ctx, &current_plugin, cross_cap)?;
    Ok(current_plugin)
}
