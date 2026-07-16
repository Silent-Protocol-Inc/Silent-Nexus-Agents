fn main() {
    println!("cargo:rerun-if-env-changed=SNX_BUILD_COMMIT");
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");

    let target = std::env::var("TARGET").unwrap_or_else(|_| "unknown".into());
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "unknown".into());
    let commit = std::env::var("SNX_BUILD_COMMIT").unwrap_or_else(|_| "development".into());
    let epoch = std::env::var("SOURCE_DATE_EPOCH").unwrap_or_else(|_| "unreproducible".into());

    println!("cargo:rustc-env=SNX_BUILD_TARGET={target}");
    println!("cargo:rustc-env=SNX_BUILD_PROFILE={profile}");
    println!("cargo:rustc-env=SNX_BUILD_COMMIT={commit}");
    println!("cargo:rustc-env=SNX_BUILD_EPOCH={epoch}");
}
