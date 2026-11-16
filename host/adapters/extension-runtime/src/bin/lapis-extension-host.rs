fn main() {
    if let Err(error) =
        extension_runtime::serve(std::io::BufReader::new(std::io::stdin()), std::io::stdout())
    {
        eprintln!("extension host: {error}");
        std::process::exit(1);
    }
}
