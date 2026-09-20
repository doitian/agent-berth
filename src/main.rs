fn main() {
    if let Err(err) = agent_bridge::run() {
        eprintln!("{err:#}");
        std::process::exit(1);
    }
}
