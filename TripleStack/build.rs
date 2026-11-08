fn main() {
    println!("cargo:rerun-if-changed=../icon.ico");
    println!("cargo:rerun-if-changed=triplestack.rc");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("triplestack.rc", embed_resource::NONE)
            .manifest_optional()
            .expect("compile the Triple Stack icon");
    }
}
