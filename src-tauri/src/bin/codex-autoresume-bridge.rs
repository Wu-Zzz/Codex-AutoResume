fn main() {
    let code = match codex_autoresume::desktop_launch::run_wrapper() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("Codex AutoResume: {error}");
            1
        }
    };
    std::process::exit(code);
}
