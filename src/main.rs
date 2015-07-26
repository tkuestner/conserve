extern crate conserve;
extern crate docopt;
#[macro_use]
extern crate log;
extern crate rustc_serialize;

use docopt::Docopt;

static USAGE: &'static str = "
Usage:
    conserve init <dir>
    conserve --version

Options:
    --version      Show version.
";

#[derive(RustcDecodable)]
struct Args {
    cmd_init: bool,
    arg_dir: String,
}


use log::{LogRecord, LogLevel, LogLevelFilter, LogMetadata};

struct SimpleLogger;

impl log::Log for SimpleLogger {
    fn enabled(&self, metadata: &LogMetadata) -> bool {
        metadata.level() <= LogLevel::Info
    }

    fn log(&self, record: &LogRecord) {
        if self.enabled(record.metadata()) {
            println!("{} - {}", record.level(), record.args());
        }
    }
}


fn run_init(args: &Args) {
    conserve::Archive::init(&args.arg_dir).unwrap();
}


fn main() {
    log::set_logger(|max_log_level| {
        max_log_level.set(LogLevelFilter::Info);
        Box::new(SimpleLogger)
    }).ok();
    info!("hello");

    let args: Args = Docopt::new(USAGE).unwrap()
        .version(Some(conserve::VERSION.to_string()))
        .decode()
        .unwrap_or_else(|e| e.exit());

    if args.cmd_init {
        run_init(&args)
    } else {
        error!("unknown command?")
    }
}
