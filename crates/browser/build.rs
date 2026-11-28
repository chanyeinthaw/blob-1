fn main() {
    // CEF's shared library and resources live beside the executable in the
    // distribution directory. Search there before system library paths.
    #[cfg(target_os = "linux")]
    println!("cargo::rustc-link-arg=-Wl,-rpath,$ORIGIN");
}
