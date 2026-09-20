fn main() {
    if let Err(err) = agent_berth::run() {
        eprintln!("{err:#}");
        std::process::exit(1);
    }
}
