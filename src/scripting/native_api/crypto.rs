use crate::scripting::models::NativeContext;
use rhai::Engine;

/// Registers cryptographic native functions with the Rhai engine.
pub fn register(engine: &mut Engine, _n_ctx: &NativeContext) {
    engine.register_fn("base64_encode", |text: &str| -> String {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(text)
    });
    engine.register_fn(
        "base64_decode",
        |text: &str| -> Result<String, Box<rhai::EvalAltResult>> {
            use base64::Engine;
            let b = base64::engine::general_purpose::STANDARD
                .decode(text)
                .map_err(|e| e.to_string())?;
            String::from_utf8(b).map_err(|e| e.to_string().into())
        },
    );
    engine.register_fn("md5", |text: &str| -> String {
        use md5::{Digest, Md5};
        let mut hasher = Md5::new();
        hasher.update(text);
        hex::encode(hasher.finalize())
    });
    engine.register_fn("sha256", |text: &str| -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(text);
        hex::encode(hasher.finalize())
    });
    engine.register_fn(
        "hmac_sha256",
        |key: &str, text: &str| -> Result<String, Box<rhai::EvalAltResult>> {
            use hmac::{Hmac, KeyInit, Mac};
            use sha2::Sha256;
            let mut mac =
                Hmac::<Sha256>::new_from_slice(key.as_bytes()).map_err(|e| e.to_string())?;
            mac.update(text.as_bytes());
            Ok(hex::encode(mac.finalize().into_bytes()))
        },
    );
}
