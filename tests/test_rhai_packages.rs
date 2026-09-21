use chaosnexus_anvil::scripting::engine::{setup_engine, empty_context};
use rhai::Dynamic;

#[test]
fn test_rhai_packages() {
    let engine = setup_engine(empty_context());
    
    // Test rhai-sci (e.g. math functions)
    let result: Dynamic = engine.eval("sin(3.14159)").unwrap();
    println!("Sci Result: {:?}", result);
    
    // Test rhai-rand
    let result: Dynamic = engine.eval("rand_float()").unwrap();
    println!("Rand Result: {:?}", result);
    
    // Test rhai-ml (e.g. standard ML functions)
    // We'll just see if it doesn't fail parsing.
}
