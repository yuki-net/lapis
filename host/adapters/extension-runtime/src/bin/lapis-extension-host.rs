fn main() {
    if let Err(error) = extension_runtime::serve(std::io::stdin().lock(), std::io::stdout().lock())
    {
        eprintln!("extension host: {error}");
        std::process::exit(1);
    }
}
