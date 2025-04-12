#![warn(clippy::all, clippy::pedantic)]
#![allow(clippy::wildcard_imports)]

mod escape;

use std::ffi::OsStr;
use std::io::{self, Write};
// TODO querycmd out of lib
use clap::{arg, command, value_parser, Args, Command, FromArgMatches as _, Parser, Subcommand};
use clap_complete::{generate, Shell};
use std::path::PathBuf;
use url::Url;

use trctl::client::{QueryCmd, Sort, TorrentAction, TorrentCli};
use trctl::config::{Builder, BuilderOpts, Config};
use trctl::console::{DefLog, Logger};
use trctl::errors::*;
use trctl::{AddArgs, TorrentLoc};

const NAME: &str = env!("CARGO_PKG_NAME");

#[derive(Parser, Debug)]
#[command()]
pub struct Cli {
    /// Sen verbosity
    #[arg(long, short, action = clap::ArgAction::Count)]
    pub verbose: u8,
    /// Run with a mock rpc client
    #[arg(long)]
    pub mock: bool,
    /// Don't ask for confirmation
    #[arg(long, short)]
    pub yes: bool,
    #[command(subcommand)]
    pub cmd: Option<CliSub>,
}

// See https://docs.rs/clap/latest/clap/_derive/index.html#terminology
#[derive(Subcommand, Debug)]
pub enum CliSub {
    /// Add torrent file
    Add {
        /// Download directory
        #[arg(long)]
        dldir: Option<PathBuf>,
        /// Whether the torrent already has files in the dldir
        #[arg(long)]
        existing: bool,
        /// Path to the torrent file
        path: Vec<PathBuf>,
    },
    /// Add magnet link or a torrent file from url
    AddUrl {
        /// Download directory
        #[arg(long)]
        dldir: Option<PathBuf>,
        /// Whether the torrent already has files in the dldir
        #[arg(long)]
        existing: bool,
        /// Url to a torrent file or a magnet link
        url: Vec<Url>,
    },
    /// Query torrents
    #[command(aliases = &["q", "qu", "que", "quer"])]
    Query(QueryCmd),
    /// Remove torrent and its data
    Rm(QueryCmd),
    /// Remove torrent but leave downloaded data in place
    Erase(QueryCmd),
    /// Clean finished torrents
    Clean(QueryCmd),
    #[command(hide(true))]
    GenCompletions {
        /// Shell the completions are generated for
        shell: Shell,
    },
    #[command(hide(true))]
    GenTorrents(QueryCmd),
    /// Move torrents with the transmission rpc call
    SetLocation {
        #[command(flatten)]
        query_opts: QueryCmd,
        /// Move files (or find them in a new location)
        #[arg(long)]
        mv: bool,
        /// New location
        #[arg(long)]
        location: PathBuf,
    },
    /// Move torrents
    Mv {
        #[command(flatten)]
        query_opts: QueryCmd,
        /// Destination directory
        #[arg(long, short)]
        destination: Option<PathBuf>,
        /// Move even if the destination directory is low on disk space
        #[arg(long, short)]
        force: bool,
        /// Verify the files after move
        #[arg(long)]
        verify: Option<bool>,
    },
    /// Queue torrents
    Start(QueryCmd),
    /// Stop torrents
    Stop(QueryCmd),
    /// Start torrents without queuing
    StartNow(QueryCmd),
    /// Verify torrents
    Verify(QueryCmd),
    /// Reannounce torrents
    Reannounce(QueryCmd),
    /// List all trackers used by the torrents
    ListTrackers(QueryCmd),
}

#[allow(clippy::too_many_lines)]
fn run<C: TorrentCli>(
    builder: Builder<C>,
    cli: Cli,
    opts: &CustomOpts,
    mut log: DefLog,
) -> Result<()> {
    if let Some(cmd) = cli.cmd {
        match cmd {
            CliSub::Add {
                dldir,
                path,
                existing,
            } => {
                let mut t = builder.new_trmv(log)?;
                let mut errors = 0;
                for p in path {
                    if let Err(err) = t.add(&AddArgs {
                        location: &TorrentLoc::Path(p),
                        dldir: dldir.as_ref(),
                        use_existing: existing,
                    }) {
                        if err.downcast_ref::<NothingToDo>().is_some() {
                            t.view.log.print_result(&Err(err)).context("log")?;
                            errors += 1;
                        } else {
                            return Err(err);
                        }
                    }
                }
                if errors > 0 {
                    bail!(Multiple(errors))
                }
                Ok(())
            }
            CliSub::AddUrl {
                dldir,
                url,
                existing,
            } => {
                let mut t = builder.new_trmv(log)?;
                let mut errors = 0;
                for u in url {
                    if let Err(err) = t.add(&AddArgs {
                        location: &TorrentLoc::Url(u),
                        dldir: dldir.as_ref(),
                        use_existing: existing,
                    }) {
                        if err.downcast_ref::<NothingToDo>().is_some() {
                            t.view.log.print_result(&Err(err)).context("log")?;
                            errors += 1;
                        } else {
                            return Err(err);
                        }
                    }
                }
                if errors > 0 {
                    bail!(Multiple(errors))
                }
                Ok(())
            }
            CliSub::SetLocation {
                query_opts,
                location,
                mv,
            } => builder.new_trctl(log)?.set_location(
                &query_opts,
                mv,
                location.to_string_lossy().to_string(),
            ),
            CliSub::Mv {
                query_opts,
                destination,
                force,
                verify,
            } => builder.new_trctl(log)?.mv(
                &query_opts,
                destination.as_ref(),
                force,
                verify,
                &opts.config,
            ),
            CliSub::Query(args) => builder.new_trctl(log)?.query(&args),
            CliSub::ListTrackers(args) => builder.new_trctl(log)?.list_trackers(&args),
            CliSub::Rm(args) => builder.new_trctl(log)?.erase(args, true),
            CliSub::Erase(args) => builder.new_trctl(log)?.erase(args, false),
            CliSub::Clean(mut args) => {
                args.cleanable = true;
                builder.new_trctl(log)?.erase(args, false)
            }
            CliSub::Verify(args) => builder.new_trctl(log)?.action(&args, TorrentAction::Verify),
            CliSub::Start(args) => builder.new_trctl(log)?.action(&args, TorrentAction::Start),
            CliSub::StartNow(args) => builder
                .new_trctl(log)?
                .action(&args, TorrentAction::StartNow),
            CliSub::Stop(args) => builder.new_trctl(log)?.action(&args, TorrentAction::Stop),
            CliSub::Reannounce(args) => builder
                .new_trctl(log)?
                .action(&args, TorrentAction::Reannounce),
            CliSub::GenTorrents(mut args) => {
                //println!("{:?}", args.strs);
                let mut client = builder.new_client()?;
                // to allow match the latest one easily
                args.reverse = true;
                args.sort = Some(Sort::Id);
                // TODO required fields
                let torrents = match client.torrent_query_sort(None, &args) {
                    Ok(torrents) => torrents,
                    Err(err) => {
                        if let Some(NoMatches) = err.downcast_ref::<NoMatches>() {
                            std::process::exit(1)
                        }
                        return Err(err);
                    }
                };
                for t in torrents {
                    if let Some(ref name) = t.name {
                        let dt = trctl::display::Torrent {
                            torrent: &t,
                            base_dir: &builder.cfg.base_dir,
                        };
                        let res = writeln!(
                            log.out(),
                            "{}:{:4} {}{} ({}%) {}/",
                            escape::zsh(name),
                            dt.id(),
                            dt.downloaded_size(),
                            dt.error_mark(),
                            dt.percent_done(),
                            dt.download_dir()
                        );
                        if let Err(err) = res {
                            return Err(err.into());
                        }
                    }
                }
                if let Err(err) = log.out().flush() {
                    return Err(err.into());
                }
                Ok(())
            }
            CliSub::GenCompletions { .. } => {
                bail!("should not happen");
            }
        }
    } else if let CliSub::Query(args) = Cli::parse_from([NAME, "query"].iter()).cmd.unwrap() {
        builder.new_trctl(log)?.query(&args)
    } else {
        panic!("bug!");
    }
}

#[derive(Args)]
struct CustomOpts {
    config: PathBuf,
}

fn build_cli() -> Result<Command> {
    let parser = command!().version(env!("BUILD_FULL_VERSION"));
    let default_cfgpath: &'static OsStr =
        Box::leak(Config::config_path(NAME)?.into_boxed_path()).as_os_str();
    let parser = parser.arg(
        arg!(-c --config <CONFIG> "Configuration file")
            .value_parser(value_parser!(PathBuf))
            .default_value(default_cfgpath),
    );

    Ok(Cli::augment_args(parser))
}

fn run_logged() -> Result<()> {
    let parser = build_cli()?;
    let matches = parser.get_matches();
    let cli = Cli::from_arg_matches(&matches)?;
    let opts = CustomOpts::from_arg_matches(&matches)?;

    let cfg = Config::load_path(&opts.config)?;

    let log = DefLog::from_choice(cfg.color, cli.verbose);
    if std::env::var("RUST_LOG").is_ok() {
        log.register_debug();
    }

    if let Some(CliSub::GenCompletions { shell }) = cli.cmd {
        generate(shell, &mut build_cli()?, NAME, &mut io::stdout());
        return Ok(());
    }

    let builder_opts = BuilderOpts {
        interactive: !cli.yes,
    };

    if cli.mock {
        let mut builder = cfg.builder_with(Builder::mock_client, NAME.to_string());
        builder.set_cli_opts(builder_opts);
        run(builder, cli, &opts, log)
    } else {
        let mut builder = cfg.builder(NAME);
        builder.set_cli_opts(builder_opts);
        run(builder, cli, &opts, log)
    }
}

fn main() -> ! {
    let mut log = DefLog::default();
    let res = run_logged();
    log.handle_exit(&res);
}

// https://rust-cli.github.io/book/index.html
// https://doc.rust-lang.org/stable/rust-by-example/conversion/string.html
// https://doc.rust-lang.org/stable/rust-by-example/fn/methods.html
// https://rust-lang.github.io/async-book/
// https://docs.rs/transmission-rpc/0.3.0/transmission_rpc/struct.TransClient.html
// https://doc.rust-lang.org/book/ch09-02-recoverable-errors-with-result.html
//
// https://kornel.ski/rust-c-speed
// https://rust-lang-nursery.github.io/rust-cookbook/algorithms/sorting.html
// https://blog.logrocket.com/a-practical-guide-to-async-in-rust/
//
// https://doc.rust-lang.org/cargo/commands/cargo-build.html
//
// https://github.com/brson/stdx
// https://lib.rs/command-line-interface
// https://github.com/vitiral/stdcli
// https://tgvashworth.com/2018/01/22/rust-cli-notes.html
// https://crates.io/categories/command-line-interface
//
// https://crates.io/crates/read_input
// https://crates.io/crates/cmd_lib
// https://crates.io/crates/paris
// https://cdn.rawgit.com/nabijaczleweli/trivial-colours-rs/doc/trivial_colours/struct.Reset.html
//
// https://doc.rust-lang.org/nomicon/borrow-splitting.html
// https://www.reddit.com/r/rust/comments/91rliw/how_to_keep_both_a_string_and_its_slice_in_a/
//
// https://rust-embedded.github.io/book/peripherals/singletons.html
//
// https://doc.rust-lang.org/edition-guide/introduction.html
// https://doc.rust-lang.org/reference/trait-bounds.html
// https://stackoverflow.com/questions/27831944/how-do-i-store-a-closure-in-a-struct-in-rust
//
// https://doc.rust-lang.org/book/ch19-05-advanced-functions-and-closures.html
//
// https://doc.rust-lang.org/std/primitive.reference.html#method.deref
//
// https://www.nayuki.io/res/bittorrent-bencode-format-tools/decode-bencode-demo.rs
// https://www.nayuki.io/res/bittorrent-bencode-format-tools/bencode-test.rs
// https://www.nayuki.io/res/bittorrent-bencode-format-tools/bencode.rs
//
//https://users.rust-lang.org/t/how-to-inspect-hir-or-mir/37135/3
//
//https://danielkeep.github.io/tlborm/book/index.html
//
// https://www.reddit.com/r/rust/comments/5rkquz/why_can_we_assign_to_a_field_of_an_instance_of_a/
