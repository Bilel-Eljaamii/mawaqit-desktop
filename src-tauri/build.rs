fn main() {
    // The legacy STATIC_VCRUNTIME env var (a Tauri v1 mechanism) triggers a
    // deprecation warning in tauri-build whenever it is present — including
    // set-but-empty — even on Linux builds. Nothing in this repo sets it;
    // it leaks from developer shells (e.g. left over from Windows
    // cross-build experiments). Static VC linking is already the tauri 2
    // schema default, so dropping the variable changes nothing and keeps
    // every environment's build output clean.
    std::env::remove_var("STATIC_VCRUNTIME");
    tauri_build::build()
}
