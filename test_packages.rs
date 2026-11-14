use rhai::Engine;
use rhai::packages::Package;

fn main() {
    let mut engine = Engine::new();
    let ml = rhai_ml::MLPackage::new();
    let bigint = rhai_bigint::BigIntPackage::new();
    let sci = rhai_sci::SciPackage::new();
    let rand = rhai_rand::RandomPackage::new();
    println!("Compiled!");
}
