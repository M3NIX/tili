fn main() {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    let result = if arguments
        .first()
        .is_some_and(|argument| argument == "--ssh-picker")
    {
        tili::ssh_hosts::run(&arguments[1..], &mut std::io::stdout().lock())
    } else {
        tili::app::run()
    };
    if let Err(error) = result {
        eprintln!("tili: {error:#}");
        std::process::exit(1);
    }
}
