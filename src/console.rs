#![allow(clippy::module_name_repetitions)]

use crate::client::{TorrentAction, TorrentAdded};
use crate::display::{ByteSize, Torrent as DisplayTorrent};
use crate::errors::*;
use crate::TorrentAddResult;
use notify_rust::{Hint, Notification, Timeout, Urgency};
use std::borrow::Borrow;
use std::fmt;
use std::io::{stdin, BufWriter, Stderr, Stdin, Stdout, Write};
use std::path::PathBuf;
use std::time::{Duration, SystemTime};
use termcolor::{BufferedStandardStream, Color, ColorChoice, ColorSpec, WriteColor};
use time::{macros::format_description, OffsetDateTime};
use tracing::{event, span, Level};
use tracing_subscriber::filter::{EnvFilter, LevelFilter};
use transmission_rpc::types::Torrent;

pub use imps::ReadLine;

#[derive(Copy, Clone)]
pub enum ConfirmAction {
    All,
    One,
}

#[macro_export]
macro_rules! print_log {
    ($target:expr, $lvl:expr, $($arg:tt)+) => ({
        let lvl = $lvl;
        if lvl <= log::STATIC_MAX_LEVEL && lvl <= Logger::max_level($target) {
            Logger::log($target,
                format_args!($($arg)+),
                lvl
            )
        } else {
            Ok(())
        }
    })
}

#[macro_export]
macro_rules! print_error {
    ($target:expr, $($arg:tt)+) => (
        print_log!($target, log::Level::Error, $($arg)+)
    )
}

#[macro_export]
macro_rules! print_warn {
    ($target:expr, $($arg:tt)+) => (
        print_log!($target, log::Level::Warn, $($arg)+)
    )
}

#[macro_export]
macro_rules! print_info {
    ($target:expr, $($arg:tt)+) => (
        print_log!($target, log::Level::Info, $($arg)+)
    )
}

#[macro_export]
macro_rules! print_debug {
    ($target:expr, $($arg:tt)+) => (
        print_log!($target, log::Level::Debug, $($arg)+)
    )
}

fn strftime(time: u64) -> Result<String> {
    let d = SystemTime::UNIX_EPOCH + Duration::from_secs(time);
    let format = format_description!("[year]-[month]-[day]");
    Ok(OffsetDateTime::from(d).format(&format)?)
}

pub mod imps {
    use std::io::{Result, Stdin};

    pub trait ReadLine {
        // need mut when not using stdin
        fn read_line(&mut self, buf: &mut String) -> Result<usize>;
    }
    impl ReadLine for Stdin {
        fn read_line(&mut self, buf: &mut String) -> Result<usize> {
            Stdin::read_line(self, buf)
        }
    }

    #[cfg(test)]
    pub mod tests {
        use termcolor::Buffer;

        use super::super::{Console, ReadLine, StdLog};
        use std::io::Result;

        pub type MockCon = Console<Buffer, MockReader>;
        pub type MockView = StdLog<Buffer>;

        impl Default for MockView {
            fn default() -> Self {
                Self {
                    out: Buffer::no_color(),
                    err: Buffer::no_color(),
                    indent: 0,
                    level: log::LevelFilter::Info,
                }
            }
        }

        pub struct MockReader {
            pub input: String,
            pub input_pos: usize,
        }
        impl ReadLine for MockReader {
            fn read_line(&mut self, buf: &mut String) -> Result<usize> {
                let mut len = 0;
                for c in self.input.chars().skip(self.input_pos) {
                    buf.push(c);
                    len += 1;
                    if c == '\n' {
                        break;
                    }
                }
                self.input_pos += len;
                Ok(len)
            }
        }
    }
}

pub struct Unprivileged {
    pub hostname: String,
    pub from: String,
    pub to: String,
    pub name: String,
}

impl Unprivileged {
    pub fn new(touser: &str, name: String) -> Result<Self> {
        let hoststring = hostname::get()?;
        let fromuserstring = whoami::username_os();
        let fromuser = fromuserstring
            .to_str()
            .ok_or_else(|| anyhow!("weird username"))?;
        let hostname = hoststring.to_str().unwrap_or("localhost").to_string();
        let from = format!("{fromuser}@{hostname}.localdomain");
        let to = format!("{touser}@{hostname}.localdomain");
        Ok(Self {
            hostname,
            from,
            to,
            name,
        })
    }
}

impl NotifyView for Unprivileged {
    fn notify(&self, urgency: Urgency, subject: &str, msg: Option<&str>) -> Result<()> {
        use lettre::transport::sendmail::SendmailTransport;
        use lettre::Message;
        use lettre::Transport;

        let prio = match urgency {
            Urgency::Low => "low",
            Urgency::Normal => "normal",
            Urgency::Critical => "critical",
        };

        let email = Message::builder()
            .from(self.from.parse()?)
            .to(self.to.parse()?)
            .subject(format!("{prio}: {subject}"))
            .body(String::from(msg.unwrap_or("<nomsg>")))?;

        let mailer = SendmailTransport::new();
        mailer.send(&email)?;
        Ok(())
    }

    fn ask_retry(&mut self, _err: &anyhow::Error) -> Result<bool> {
        unimplemented!();
    }

    fn ask_existing(&mut self, torrent: &[u8], modified: u64) -> Result<bool> {
        use native_dialog::{MessageDialog, MessageType};
        let msg = format!(
            "Download again:\n{}\nmodified: {}",
            &String::from_utf8_lossy(torrent),
            strftime(modified)?,
        );
        let dialog = MessageDialog::new()
            .set_title(&self.name)
            .set_text(&msg)
            .set_type(MessageType::Info);
        Ok(dialog.show_confirm()?)
    }
}

pub struct Dbus {
    pub name: String,
    pub icon: String,
    pub v_ask_existing: bool,
    app_name_override: String,
}

impl Dbus {
    #[must_use]
    #[allow(clippy::missing_panics_doc)]
    pub fn new(name: String, v_ask_existing: bool) -> Self {
        // BUG To avoid  https://gitlab.gnome.org/GNOME/libnotify/-/issues/41
        let app_name = std::env::current_exe()
            .unwrap()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        let app_name_override = format!("{app_name}-notify");
        Self {
            // awesome wm needs absolute path or it looks the file in home dir first
            icon: format!("/usr/share/pixmaps/{name}.png"),
            name,
            v_ask_existing,
            app_name_override,
        }
    }
}

impl NotifyView for Dbus {
    fn ask_retry(&mut self, err: &anyhow::Error) -> Result<bool> {
        let mut ret = false;

        Notification::new()
            .summary("Failed, retry?")
            .body(&format!("{err:#}"))
            .hint(Hint::Resident(true))
            .hint(Hint::ActionIcons(true))
            .action("yes", "yes")
            .action("no", "no")
            .timeout(Timeout::Never)
            .icon(&self.icon) // TODO
            .show()
            .context("notify failed")?
            .wait_for_action(|action| match action {
                "yes" => ret = true,
                // "__closed"| "no" | ??
                _ => ret = false,
            });

        Ok(ret)
    }

    fn notify(&self, urgency: Urgency, summary: &str, body: Option<&str>) -> Result<()> {
        let mut noti = Notification::new();
        noti.summary(&format!("{}: {}", self.name, summary))
            .urgency(urgency)
            .appname(&self.app_name_override)
            .icon(&self.icon); // TODO
        if let Some(body) = body {
            noti.body(body);
        }
        noti.show().context("notify failed")?;
        Ok(())
    }

    fn ask_existing(&mut self, torrent: &[u8], modified: u64) -> Result<bool> {
        if !self.v_ask_existing {
            return Ok(true);
        }

        let mut ret = false;

        Notification::new()
            .summary("Duplicate, download again?")
            .body(&format!(
                "[{}]: {}",
                strftime(modified)?,
                &String::from_utf8_lossy(torrent),
            ))
            .hint(Hint::Resident(true))
            .hint(Hint::ActionIcons(true))
            .action("yes", "yes")
            .action("no", "no")
            .timeout(Timeout::Never)
            .icon(&self.icon) // TODO
            .show()
            .context("notify failed")?
            .wait_for_action(|action| match action {
                "yes" => ret = true,
                // "__closed"| "no" | ??
                _ => ret = false,
            });

        Ok(ret)
    }
}

pub trait NotifyView {
    fn notify(&self, urgency: Urgency, summary: &str, body: Option<&str>) -> Result<()>;
    fn ask_existing(&mut self, name: &[u8], modified: u64) -> Result<bool>;
    fn ask_retry(&mut self, err: &anyhow::Error) -> Result<bool>;
}
pub struct Notifier<NV: NotifyView> {
    notify_view: NV,
    pub out: BufWriter<Stdout>,
    pub err: BufWriter<Stderr>,
    pub name: String,
}

impl<NV: NotifyView> Notifier<NV> {
    pub fn new(notify_view: NV, name: String) -> Self {
        Self {
            notify_view,
            out: BufWriter::new(std::io::stdout()),
            err: BufWriter::new(std::io::stderr()),
            name,
        }
    }

    fn do_log(&mut self, args: fmt::Arguments, level: log::Level, send: bool) -> Result<()> {
        let (prefix, summary) = match level {
            log::Level::Debug | log::Level::Trace => ("7", None),
            log::Level::Info => ("6", None),
            log::Level::Warn => ("4", None),
            log::Level::Error => ("3", Some("Error")),
        };
        writeln!(self.out, "<{}>{}: {}", prefix, self.name, args)?;
        self.out.flush()?;
        if send {
            if let Some(summary) = summary {
                return self.notify_view.notify(
                    Urgency::Critical,
                    summary,
                    Some(&format!("{args}")),
                );
            }
        }
        Ok(())
    }
}

impl<NV: NotifyView> View for Notifier<NV> {
    type Logger = Self;

    fn ask_retry(&mut self, err: &anyhow::Error) -> Result<bool> {
        self.notify_view.ask_retry(err)
    }

    fn ask_existing(&mut self, torrent: &[u8], modified: u64) -> Result<bool> {
        self.notify_view.ask_existing(torrent, modified)
    }

    fn torrent_action_ok<IT>(&mut self, _torrents: IT, _action: Action) -> Result<()>
    where
        IT: IntoIterator,
        IT::Item: Borrow<Torrent>,
    {
        panic!("TODO");
    }

    fn torrent_add_result(&mut self, res: &TorrentAddResult) -> Result<()> {
        match &res.response {
            TorrentAdded::TorrentAdded { name, .. } => {
                let mut msg = format!(
                    "Torrent added (T{} F{})",
                    ByteSize(res.total_size),
                    ByteSize(res.left)
                );
                if res.exists.is_some() {
                    msg.push_str(" [have]");
                }
                if res.full {
                    msg.push_str(" [full]");
                }
                self.notify_view.notify(
                    Urgency::Normal,
                    &msg,
                    Some(name.as_ref().ok_or_else(|| anyhow!("no name"))?),
                )
            }
            TorrentAdded::TorrentDuplicate { id, name, .. } => self.notify_view.notify(
                Urgency::Normal,
                &format!(
                    "Already loaded ({}) (id: {})",
                    if res.exists.is_some() {
                        "completed"
                    } else {
                        "incomplete"
                    },
                    id.ok_or_else(|| anyhow!("no id"))?
                ),
                Some(name.as_ref().ok_or_else(|| anyhow!("no name"))?),
            ),
        }
    }

    fn log(&mut self) -> &mut Self::Logger {
        self
    }
}

impl<NV: NotifyView> Logger for Notifier<NV> {
    fn print_result(&mut self, res: &Result<()>) -> Result<()> {
        match res {
            Ok(()) => Ok(()),
            Err(err) => {
                if let Some(NothingToDo(msg)) = err.downcast_ref::<NothingToDo>() {
                    print_warn!(self, "{}", msg).context("log")
                } else if let Some(msg) = err.downcast_ref::<NoMatches>() {
                    print_warn!(self, "{}", msg).context("log")
                } else if let Some(msg) = err.downcast_ref::<Multiple>() {
                    print_warn!(self, "{}", msg).context("log")
                } else {
                    self.do_log(format_args!("{err:#}"), log::Level::Error, false)
                        .context("log")?;
                    self.notify_view
                        .notify(Urgency::Critical, "error", Some(&format!("{err:#}")))
                        .context("log")
                }
            }
        }
    }

    fn log(&mut self, args: fmt::Arguments, level: log::Level) -> Result<()> {
        let span = span!(Level::TRACE, "log");
        let _guard = span.enter();
        event!(Level::TRACE, "logging");

        self.do_log(args, level, true)
    }

    fn add_indent(&mut self) {}

    fn pop_indent(&mut self) {}

    fn max_level(&self) -> log::LevelFilter {
        log::LevelFilter::Info
    }
}

// No user interaction expected
pub trait Logger {
    fn max_level(&self) -> log::LevelFilter;

    fn log(&mut self, args: fmt::Arguments, level: log::Level) -> Result<()>;

    fn register_debug(&self) {
        //env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("trctl=info"))
        //    .parse_default_env()
        //    .init();
        let filter = EnvFilter::builder()
            .with_default_directive(LevelFilter::INFO.into())
            .from_env_lossy();
        tracing_subscriber::fmt().with_env_filter(filter).init();
    }

    fn add_indent(&mut self);

    fn pop_indent(&mut self);

    fn print_result(&mut self, res: &Result<()>) -> Result<()>;

    fn handle_exit(&mut self, res: &Result<()>) -> ! {
        let span = span!(Level::TRACE, "handle_exit");
        let _guard = span.enter();

        event!(Level::DEBUG, "exiting [{res:?}]");
        self.print_result(res).unwrap();
        match res {
            Ok(()) => std::process::exit(0),
            Err(err) => {
                if let Some(NothingToDo(_)) = err.downcast_ref::<NothingToDo>() {
                    std::process::exit(0);
                } else {
                    std::process::exit(1);
                }
            }
        }
    }
}

// User interaction required
pub trait View {
    type Logger: Logger;

    fn ask_retry(&mut self, err: &anyhow::Error) -> Result<bool>;

    fn ask_existing(&mut self, name: &[u8], modified: u64) -> Result<bool>;

    fn torrent_add_result(&mut self, res: &TorrentAddResult) -> Result<()>;

    fn torrent_action_ok<IT>(&mut self, torrents: IT, action: Action) -> Result<()>
    where
        IT: IntoIterator,
        IT::Item: Borrow<Torrent>;

    fn log(&mut self) -> &mut Self::Logger;
}

pub type DefCon = Console<BufferedStandardStream, Stdin>;
pub type DefLog = StdLog<BufferedStandardStream>;

#[derive(Debug, Clone)]
pub struct StdLog<O: WriteColor> {
    pub out: O,
    pub err: O,
    pub indent: usize,
    pub level: log::LevelFilter,
}

impl<O: WriteColor> StdLog<O> {
    pub fn out(&mut self) -> &mut O {
        &mut self.out
    }
    pub fn err(&mut self) -> &mut O {
        &mut self.err
    }

    #[must_use]
    pub fn new<U: WriteColor>(out: U, err: U, level: log::LevelFilter) -> StdLog<U> {
        StdLog {
            out,
            err,
            indent: 0,
            level,
        }
    }

    #[must_use]
    pub fn from_choice(want: Option<bool>, verbosity: u8) -> StdLog<BufferedStandardStream> {
        let choice = if let Some(b) = want {
            if b {
                ColorChoice::Always
            } else {
                ColorChoice::Never
            }
        } else {
            ColorChoice::Auto
        };
        let level = match verbosity {
            0 => log::LevelFilter::Info,
            1 => log::LevelFilter::Debug,
            _ => log::LevelFilter::Trace,
        };
        Self::new(
            BufferedStandardStream::stdout(choice),
            BufferedStandardStream::stderr(choice),
            level,
        )
    }
}

impl Default for StdLog<BufferedStandardStream> {
    fn default() -> Self {
        Self::from_choice(None, 0)
    }
}

#[cfg(test)]
use termcolor::Buffer;
#[cfg(test)]
impl StdLog<Buffer> {
    pub fn to_string(&mut self) -> Result<String> {
        let mut out = String::from_utf8(self.out.as_slice().to_vec())?;
        let err = String::from_utf8(self.err.as_slice().to_vec())?;
        out.push_str(&err);
        Ok(out)
    }
}

#[derive(Debug)]
pub struct Console<O: WriteColor, I: ReadLine> {
    pub log: StdLog<O>,
    pub base_dir: PathBuf,
    pub input: I,
    pub v_ask_existing: bool,
}

pub enum Action {
    TorrentAction(TorrentAction),
    SetLocation { moved: bool },
}
impl<O: WriteColor, I: ReadLine> View for Console<O, I> {
    type Logger = StdLog<O>;

    fn ask_retry(&mut self, _err: &anyhow::Error) -> Result<bool> {
        unimplemented!();
    }

    fn ask_existing(&mut self, name: &[u8], modified: u64) -> Result<bool> {
        if !self.v_ask_existing {
            return Ok(true);
        }

        self.yesno(&format!(
            "'{}' exists (modified {}). Download again",
            &String::from_utf8_lossy(name),
            strftime(modified)?,
        ))
    }

    fn log(&mut self) -> &mut Self::Logger {
        &mut self.log
    }

    // TODO print torrent names without quieying them because no changes are seen yet anyway
    fn torrent_action_ok<IT>(&mut self, torrents: IT, action: Action) -> Result<()>
    where
        IT: IntoIterator,
        IT::Item: Borrow<Torrent>,
    {
        match action {
            Action::TorrentAction(TorrentAction::Reannounce) => {
                print_info!(&mut self.log, "Reannouncing:")?;
            }
            Action::TorrentAction(TorrentAction::Start) => print_info!(&mut self.log, "Started:")?,
            Action::TorrentAction(TorrentAction::StartNow) => {
                print_info!(&mut self.log, "Started immediately:")?;
            }
            Action::TorrentAction(TorrentAction::Verify) => {
                print_info!(&mut self.log, "Verifying:")?;
            }
            Action::TorrentAction(TorrentAction::Stop) => print_info!(&mut self.log, "Stopped:")?,
            Action::SetLocation { moved: false } => print_info!(&mut self.log, "Location set")?,
            Action::SetLocation { moved: true } => print_info!(&mut self.log, "Torrent moved")?,
        }
        for t in torrents {
            let tor = t.borrow();
            print_info!(
                &mut self.log,
                "{}: {}",
                tor.id.unwrap_or(0),
                tor.name.as_deref().unwrap_or("no name")
            )?;
        }
        Ok(())
    }

    fn torrent_add_result(&mut self, res: &TorrentAddResult) -> Result<()> {
        match &res.response {
            TorrentAdded::TorrentAdded { name, .. } => {
                let mut status = String::new();
                if res.exists.is_some() {
                    status.push_str("have ");
                }
                if res.full {
                    status.push_str("full ");
                }
                print_info!(
                    &mut self.log,
                    "Torrent added ({}T{} F{}): {}",
                    status,
                    ByteSize(res.total_size),
                    ByteSize(res.left),
                    name.as_ref().ok_or_else(|| anyhow!("no name"))?
                )
            }
            TorrentAdded::TorrentDuplicate { id, name, .. } => print_warn!(
                &mut self.log,
                "Already loaded ({}) (id: {}): {}",
                if res.exists.is_some() {
                    "completed"
                } else {
                    "incomplete"
                },
                id.ok_or_else(|| anyhow!("no id"))?,
                name.as_ref().ok_or_else(|| anyhow!("no name"))?
            ),
        }
    }
}

impl<O: WriteColor> Logger for StdLog<O> {
    fn print_result(&mut self, res: &Result<()>) -> Result<()> {
        match res {
            Ok(()) => Ok(()),
            Err(err) => {
                if let Some(NothingToDo(msg)) = err.downcast_ref::<NothingToDo>() {
                    print_warn!(self, "{}", msg).context("log")
                } else if let Some(msg) = err.downcast_ref::<NoMatches>() {
                    print_warn!(self, "{}", msg).context("log")
                } else if let Some(msg) = err.downcast_ref::<Multiple>() {
                    print_warn!(self, "{}", msg).context("log")
                } else {
                    print_error!(self, "{:#}", err).context("log")
                }
            }
        }
    }

    fn log(&mut self, args: fmt::Arguments, level: log::Level) -> Result<()> {
        // flush any partial writes done
        self.out.flush()?;
        match level {
            log::Level::Info => {
                self.out
                    .set_color(ColorSpec::new().set_fg(Some(Color::Green)))?;
                write!(self.out, "{:indent$}", "-- ", indent = self.indent)?;
                self.out.reset()?;
                writeln!(self.out, "{args}")?;
                self.out.flush()?;
            }
            log::Level::Warn => {
                self.err
                    .set_color(ColorSpec::new().set_fg(Some(Color::Yellow)))?;
                write!(self.err, "-w ")?;
                self.err.reset()?;
                writeln!(self.err, "{args}")?;
                self.err.flush()?;
            }
            log::Level::Error => {
                self.err
                    .set_color(ColorSpec::new().set_fg(Some(Color::Red)))?;
                write!(self.err, "-e ")?;
                self.err.reset()?;
                writeln!(self.err, "{args}")?;
                self.err.flush()?;
            }
            _ => panic!("todo"),
        }
        Ok(())
    }

    fn add_indent(&mut self) {
        self.indent += 4;
    }

    fn pop_indent(&mut self) {
        if self.indent >= 4 {
            self.indent -= 4;
        }
    }

    fn handle_exit(&mut self, res: &Result<()>) -> ! {
        let span = span!(Level::TRACE, "handle_exit");
        let _guard = span.enter();

        event!(Level::DEBUG, "exiting [{res:?}]");
        if let Err(err) = self.print_result(res) {
            // It's ok not to check if the original res is a broken pipe error because we would
            // then get it again here.
            if let Some(ioe) = err.downcast_ref::<std::io::Error>() {
                if ioe.kind() == std::io::ErrorKind::BrokenPipe {
                    std::process::exit(0);
                }
            }
        }
        match res {
            Ok(()) => std::process::exit(0),
            Err(err) => {
                if let Some(NothingToDo(_)) = err.downcast_ref::<NothingToDo>() {
                    std::process::exit(0);
                } else {
                    std::process::exit(1);
                }
            }
        }
    }

    fn max_level(&self) -> log::LevelFilter {
        self.level
    }
}

impl<O: WriteColor> Console<O, Stdin> {
    pub fn new(base_dir: PathBuf, log: StdLog<O>, v_ask_existing: bool) -> Self {
        Self {
            log,
            base_dir,
            input: stdin(),
            v_ask_existing,
        }
    }
}

impl<O: WriteColor, I: ReadLine> Console<O, I> {
    pub fn out(&mut self) -> &mut impl WriteColor {
        self.log().out()
    }

    pub fn err(&mut self) -> &mut impl WriteColor {
        self.log().err()
    }

    /// Reads user input, but without \n as `stdin::read_line` would
    fn read_reply(&mut self) -> std::io::Result<String> {
        let mut reply = String::new();

        self.input.read_line(&mut reply)?;

        // We should have a newline at the end. This helps prevent things such as:
        // > printf "no-newline" | program-using-rprompt
        // If we didn't have the \n check, we'd be removing the last "e" by mistake.
        if !reply.ends_with('\n') {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "unexpected end of file",
            ));
        }

        // Remove the \n from the line.
        reply.pop();

        // Remove the \r from the line if present
        if reply.ends_with('\r') {
            reply.pop();
        }

        Ok(reply)
    }

    fn yesno(&mut self, question: &str) -> Result<bool> {
        loop {
            write!(self.log.out(), "{question} [y/N]: ")?;
            self.log.out().flush()?;

            let ans = self.read_reply()?;

            if ans == "y" {
                return Ok(true);
            } else if ans.is_empty() || ans == "n" || ans == "N" {
                return Ok(false);
            }
            print_warn!(self.log(), "Invalid selection '{ans}'")?;
        }
    }

    // Return the array ids of selected torrents
    pub fn confirm<TOR>(
        &mut self,
        torrents: &[TOR],
        action: Option<ConfirmAction>,
    ) -> crate::errors::Result<Vec<usize>>
    where
        TOR: Borrow<Torrent>,
    {
        if torrents.is_empty() {
            bail!(NoMatches);
        }
        self.print_filtered(torrents.iter().map(Borrow::borrow))
            .context("print_filtered")?;

        let mut need_one = false;
        match action {
            Some(ConfirmAction::All) => return Ok((0..torrents.len()).collect()),
            Some(ConfirmAction::One) => need_one = true,
            None => (),
        }

        if torrents.len() == 1 {
            if self.yesno("Select").context("yesno")? {
                Ok(vec![0])
            } else {
                bail!(NothingToDo("No selection"));
            }
        } else {
            loop {
                if need_one {
                    write!(self.log.out(), "Select [{{n}}/N]: ")?;
                } else {
                    write!(self.log.out(), "Select [a/{{n}}/N]: ")?;
                }
                self.log.out().flush()?;

                let ans = self.read_reply()?;

                if ans.is_empty() || ans == "n" || ans == "N" {
                    return Ok(vec![]);
                } else if !need_one && ans == "a" {
                    return Ok((0..torrents.len()).collect());
                }
                let n = ans.parse::<i64>();
                match n {
                    Err(e) => print_warn!(self.log(), "{}", e)?,
                    Ok(num) => {
                        let idx = torrents.iter().enumerate().find_map(|(i, t)| {
                            if Some(num) == t.borrow().id {
                                Some(i)
                            } else {
                                None
                            }
                        });
                        if let Some(i) = idx {
                            return Ok(vec![i]);
                        }
                        print_warn!(self.log(), "Invalid id")?;
                    }
                }
            }
        }
    }

    pub fn print_filtered<IT>(&mut self, torrents: IT) -> Result<()>
    where
        IT: IntoIterator,
        IT::Item: Borrow<Torrent>,
    {
        writeln!(self.log.out(), "{}", DisplayTorrent::get_header())?;

        let mut total_size = 0;
        let mut total_up = 0;
        let mut total_down = 0;
        for t in torrents {
            let tor = t.borrow();
            writeln!(
                self.log.out(),
                "{}",
                DisplayTorrent {
                    torrent: tor,
                    base_dir: &self.base_dir
                }
            )?;
            let downloaded_size = tor.size_when_done.and_then(|x| {
                if x < 0 {
                    None
                } else {
                    tor.left_until_done
                        .and_then(|z| if z < 0 { None } else { Some(z) })
                        .map(|y| x - y)
                }
            });
            total_size += downloaded_size.unwrap_or(0);
            total_up += tor.rate_upload.map_or(0, |x| if x < 0 { 0 } else { x });
            total_down += tor.rate_download.map_or(0, |x| if x < 0 { 0 } else { x });
        }

        writeln!(
            self.log.out(),
            "Sum:  {:14}  {:26}  {:7}",
            ByteSize(total_size),
            ByteSize(total_up),
            ByteSize(total_down)
        )?;

        self.log.out().flush()?;
        Ok(())
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    //use pretty_assertions::assert_eq;
    #[test]
    #[ignore]
    fn testmail() -> crate::errors::Result<()> {
        Unprivileged::new("hellouser", "itsme".into())?.notify(
            Urgency::Critical,
            "Torrent completed",
            Some("hello world me"),
        )?;
        Ok(())
    }

    use native_dialog::{MessageDialog, MessageType};
    #[test]
    #[ignore]
    fn dialog() {
        let dialog = MessageDialog::new()
            .set_title("Hello")
            .set_text("How are you?")
            .set_type(MessageType::Info);
        let res = dialog.show_confirm().unwrap();
        assert!(res);
    }
}
