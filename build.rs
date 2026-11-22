// Avoid injecting the wall-clock link time into Windows PE files. This is scoped
// to our own executables and does not alter dependency compilation or test runs.
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    match std::env::var("TARGET").as_deref() {
        Ok("x86_64-pc-windows-gnu") => {
            println!("cargo:rustc-link-arg-bins=-Wl,--no-insert-timestamp")
        }
        Ok("x86_64-pc-windows-msvc") => println!("cargo:rustc-link-arg-bins=/Brepro"),
        _ => {}
    }
}
