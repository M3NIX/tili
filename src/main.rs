fn main() {
    if let Err(error) = tili::app::run() {
        eprintln!("tili: {error:#}");
        std::process::exit(1);
    }
}
