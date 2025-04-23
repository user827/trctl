use clap::Parser;
use std::path::PathBuf;
use trctl::client::SyncRequest;
use trctl::config::{Builder, Config};
use trctl::console::{Logger,View as _};
//use trctl::console::Unprivileged;
use tracing::{event, span, Level};
use trctl::errors::*;
use trctl::AddArgs;
use trctl::TorrentLoc;
use url::Url;

const NAME: &str = "trmv";
const CONFIG_NAME: &str = env!("CARGO_PKG_NAME");

#[derive(Parser, Debug)]
#[command(name = NAME, about = "My trmv.")]
pub struct Cli {
    #[arg(long, short)]
    pub debug: bool,
    #[command(subcommand)]
    pub cmd: Command,
}

#[derive(Parser, Debug)]
pub enum Command {
    Add {
        #[arg(long)]
        dldir: Option<PathBuf>,
        #[arg(long)]
        existing: bool,
        path: PathBuf,
    },
    AddUrl {
        #[arg(long)]
        dldir: Option<PathBuf>,
        #[arg(long)]
        existing: bool,
        url: Url,
    },
}

fn run_logged(builder: Builder<SyncRequest>) -> Result<()> {
    let span = span!(Level::TRACE, "run_logged");
    let _guard = span.enter();

    let cli = Cli::parse();

    let log = builder.new_notifier_dbus(NAME.to_string());
    if std::env::var("RUST_LOG").is_ok() {
        log.register_debug();
    }

    let mut trmv = builder.new_trmv_view(log)?;

    use Command::*;
    let mut count = 1;
    loop {
        let res = match cli.cmd {
            Add {
                ref dldir,
                ref path,
                existing,
            } => trmv.add(&AddArgs {
                location: &TorrentLoc::Path(path.clone()),
                dldir: dldir.as_ref(),
                use_existing: existing,
            }),
            AddUrl {
                ref dldir,
                ref url,
                existing,
            } => trmv.add(&AddArgs {
                location: &TorrentLoc::Url(url.clone()),
                dldir: dldir.as_ref(),
                use_existing: existing,
            }),
        };
        count += 1;
        //println!("count: {count}");
        if let Err(ref err) = res {
            if let Some(NothingToDo(_)) = err.downcast_ref::<NothingToDo>() {
            } else if count <= 3 && trmv.view.ask_retry(err)? {
                continue
            }
        }
        return res;
    }
}

fn main() -> ! {
    let cfg = Config::load(CONFIG_NAME).expect("could not load config");
    let builder = cfg.builder(CONFIG_NAME);

    let mut log = builder.new_notifier_dbus(NAME.to_string());

    let span = span!(Level::TRACE, "main");
    let _guard = span.enter();

    let res = run_logged(builder);
    event!(Level::DEBUG, "finishing [{res:?}]");
    log.handle_exit(&res);
}
