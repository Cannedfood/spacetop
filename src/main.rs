mod launcher;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    launcher::run(std::env::args().skip(1)).map_err(Into::into)
}

#[cfg(test)]
#[path = "../tests/unit/cli.rs"]
mod cli_tests;
