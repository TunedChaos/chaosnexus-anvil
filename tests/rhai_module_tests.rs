// chaosnexus-anvil/tests/rhai_module_tests.rs
//
// Regression coverage for Rhai module imports with the project's engine setup:
// plugin-local files (relative to the loading script) and shared libraries
// under `scripts/lib/` via `import "lib/foo" as f;`.

use std::fs;
use std::path::PathBuf;

/// Plugin-local module next to the loading script (cwd = temp root).
fn setup_plugin_local_module() -> PathBuf {
    let tmp = std::env::temp_dir().join(format!("cw_rhai_mod_{}", std::process::id()));
    fs::create_dir_all(&tmp).expect("create temp dir");
    std::env::set_current_dir(&tmp).expect("enter temp dir");
    fs::write(
        tmp.join("mathlib.rhai"),
        "fn double(x) { x * 2 }\nlet PI = 3;\nexport PI;\n",
    )
    .expect("write plugin-local module");
    tmp
}

/// `scripts/lib/` tree with paths initialized (no chdir required).
fn setup_shared_lib_tree() -> PathBuf {
    let tmp = std::env::temp_dir().join(format!("cw_rhai_shared_{}", std::process::id()));
    let scripts = tmp.join("scripts");
    let lib = scripts.join("lib");
    fs::create_dir_all(&lib).expect("create lib dir");
    fs::write(lib.join("string_utils.rhai"), r#"fn add_ten(x) { x + 10 }"#)
        .expect("write shared lib module");
    chaosnexus_anvil::scripting::paths::init(&scripts);
    tmp
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rhai_modules_are_importable_in_the_plugin_engine() {
    let tmp = setup_plugin_local_module();
    let engine = chaosnexus_anvil::scripting::engine::setup_engine(
        chaosnexus_anvil::scripting::engine::empty_context(),
    );

    // Shape 1: top-level import evaluated as a whole script.
    let top_level_eval = engine
        .eval::<i64>(r#"import "mathlib" as m; m::double(21)"#)
        .expect("top-level import + eval should resolve the module");
    assert_eq!(top_level_eval, 42);

    // Shape 2: import INSIDE a function invoked via call_fn. This mirrors how
    // PluginManager drives plugins (compile once, then call entrypoints).
    let ast_fn_scope = engine
        .compile(r#"fn run() { import "mathlib" as m; m::double(21) }"#)
        .expect("compile function-scoped import");
    let fn_scope_import = engine
        .call_fn::<i64>(&mut rhai::Scope::new(), &ast_fn_scope, "run", ())
        .expect("function-scoped import should resolve when called via call_fn");
    assert_eq!(fn_scope_import, 42);

    // Shape 3: top-level import shared by a separately-invoked function.
    let ast_top = engine
        .compile(r#"import "mathlib" as m; fn run() { m::double(21) }"#)
        .expect("compile top-level import + function");
    let shared_import = engine
        .call_fn::<i64>(&mut rhai::Scope::new(), &ast_top, "run", ())
        .expect("top-level import should be visible to call_fn entrypoints");
    assert_eq!(shared_import, 42);

    let _ = fs::remove_dir_all(&tmp);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shared_scripts_lib_import_resolves_via_module_resolver() {
    let tmp = setup_shared_lib_tree();
    let engine = chaosnexus_anvil::scripting::engine::setup_engine(
        chaosnexus_anvil::scripting::engine::empty_context(),
    );

    let result = engine
        .eval::<i64>(r#"import "lib/string_utils" as su; su::add_ten(32)"#)
        .expect("shared lib import should resolve under scripts/lib/");
    assert_eq!(result, 42);

    let _ = fs::remove_dir_all(&tmp);
}
