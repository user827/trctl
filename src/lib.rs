#![warn(clippy::all, clippy::pedantic)]
#![allow(clippy::wildcard_imports)]
#![allow(clippy::missing_errors_doc)]

pub mod client;
pub mod config;
pub mod console;
#[cfg(feature = "sqlite")]
pub mod db;
pub mod display;
pub mod errors;
pub mod torrent;

use base64::Engine as _;
use db::DBSqlite;
use std::borrow::Borrow;
use std::convert::TryFrom as _;
use std::io::Write;
use std::iter::Iterator;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::SystemTime;
use termcolor::WriteColor;

use display::ByteSize;
use magnet_uri::MagnetURI;
use torrent::Torrent as TorrentInfo;
use tracing::{event, instrument, span, Level};
use transmission_rpc::types::{Torrent, TorrentStatus};
use url::Url;

use client::TorrentAction;
use client::{Client, QueryCmd, TorrentAddArgs, TorrentAdded, TorrentCli, TorrentFilter};
use console::{Action, ConfirmAction, Console, Logger, ReadLine, View};
#[cfg(feature = "sqlite")]
use db::DB;
use errors::*;

pub struct TorrentAddResult {
    pub response: TorrentAdded,
    pub exists: Option<u64>,
    pub full: bool,
    pub left: i64,
    pub total_size: i64,
}

#[derive(Debug)]
pub struct AddArgs<'a> {
    pub location: &'a TorrentLoc,
    pub dldir: Option<&'a PathBuf>,
    pub use_existing: bool,
}

pub struct Trmv<C: TorrentCli, V: View> {
    pub client: Client<C>,
    pub view: V,
    pub copydir: Option<PathBuf>,
    pub base_dir: PathBuf,
    pub dldirs: Vec<PathBuf>,
    pub quota: u64,
    pub safe_space: u64,
    #[cfg(feature = "sqlite")]
    pub db: DBSqlite,
}

impl<C: TorrentCli, V: View> Trmv<C, V> {
    fn get_safe_space_and_dldir(
        &mut self,
        dldir: Option<&PathBuf>,
        hsh: &str,
        use_existing: bool,
    ) -> Result<(i64, i64, PathBuf)> {
        print_debug!(self.view.log(), "hsh: {}", hsh).context("log")?;
        let mut download_dir;
        match dldir {
            None => {
                let session = self.client.session_get()?;
                download_dir = PathBuf::from(session.download_dir);
            }
            Some(d) => {
                download_dir = PathBuf::from(&self.base_dir);
                download_dir.push(d);
            }
        }
        let f = self
            .client
            .free_space(download_dir.to_string_lossy().to_string())
            .with_context(|| {
                format!("could not query free space for: {}", download_dir.display())
            })?;
        let free_space = f.size_bytes;

        let torrents = self.client.torrent_get(None, None)?;
        let mut total_size = 0;
        let mut safe_space = free_space;
        for t in torrents {
            if Path::new(t.download_dir.as_ref().context("torrent without dldir")?)
                .starts_with(&download_dir)
            {
                let mut allocated_size = 0;
                for file in t.files.as_ref().context("undefined files")? {
                    if file.bytes_completed > 0 {
                        allocated_size += file.length;
                    }
                }
                let (final_size, left_until_done) =
                    if t.status.context("undefined status")? == TorrentStatus::Stopped {
                        (allocated_size, 0)
                    } else {
                        let mut size_when_done = 0;
                        let wanted_array = t.wanted.as_ref().context("undefined wanted")?;
                        for (i, file) in t
                            .files
                            .as_ref()
                            .context("undefined files")?
                            .iter()
                            .enumerate()
                        {
                            let wanted = wanted_array[i];
                            if wanted || file.bytes_completed > 0 {
                                size_when_done += file.length;
                            }
                        }
                        (size_when_done, size_when_done - allocated_size)
                    };

                if final_size < 0 {
                    print_warn!(
                        self.view.log(),
                        "negative size_when_done/allocated_size for torrent {}",
                        t.name.as_ref().context("undefined name")?
                    )?;
                } else {
                    total_size += final_size;
                }

                if left_until_done < 0 {
                    print_warn!(
                        self.view.log(),
                        "negative left_until_done for torrent {}",
                        t.name.as_ref().context("undefined name")?
                    )?;
                } else {
                    safe_space -= left_until_done;
                }
            }
        }

        print_info!(
            self.view.log(),
            "safe space: {}, total_size: {} in {}",
            ByteSize(safe_space),
            ByteSize(total_size),
            download_dir.display(),
        )?;

        if !use_existing {
            download_dir.push(hsh);
        }
        print_debug!(self.view.log(), "download_dir: {}", download_dir.display()).context("log")?;

        Ok((safe_space, total_size, download_dir))
    }

    fn check_existing(&mut self, hsh: &str) -> Result<Option<u64>> {
        let exists_copydir = if let Some(ref copydir) = self.copydir {
            Self::check_existing_copydir(copydir, hsh)?
        } else {
            None
        };
        if exists_copydir.is_some() {
            return Ok(exists_copydir);
        }

        #[cfg(feature = "sqlite")]
        return self.db.has(hsh);
        #[cfg(not(feature = "sqlite"))]
        return Ok(None);
    }

    fn check_existing_copydir(copydir: &Path, hsh: &str) -> Result<Option<u64>> {
        let mut existing = copydir.join(hsh);
        existing.set_extension("torrent");
        match existing.metadata() {
            Err(error) => match error.kind() {
                std::io::ErrorKind::NotFound => Ok(None),
                _ => Err(anyhow!(error)).context("Copydir"),
            },
            Ok(meta) => Ok(Some(
                meta.modified()?
                    .duration_since(SystemTime::UNIX_EPOCH)?
                    .as_secs(),
            )),
        }
    }

    // Breaks completion
    #[instrument(err, level = "trace", skip(self))]
    pub fn add(&mut self, args: &AddArgs) -> Result<()> {
        let location = args.location;
        let dldir = args.dldir;
        let use_existing = args.use_existing;
        match location {
            TorrentLoc::Path(path) => {
                let content = std::fs::read(path)?;
                let torrent = TorrentInfo::from_bytes(&content).context("TorrentInfo")?;
                let hsh = torrent.info_hash;
                event!(Level::DEBUG, "got hsh [{hsh}]");
                print_debug!(self.view.log(), "info hash: {}", hsh)?;
                let exists = self.check_existing(&hsh)?;
                if let Some(time) = exists {
                    if !self.view.ask_existing(&torrent.name, time)? {
                        bail!(NothingToDo("Nothing to do"));
                    }
                }
                let (safe_space, total_size, download_dir) =
                    self.get_safe_space_and_dldir(dldir, &hsh, use_existing)?;
                print_debug!(self.view.log(), "torrent length: {}", torrent.length)?;
                let would_be_left =
                    safe_space - i64::try_from(torrent.length).context("overflow")?;
                let would_be_size =
                    total_size + i64::try_from(torrent.length).context("overflow")?;

                self.add_torrent(
                    TorrentAddArgs {
                        download_dir: Some(download_dir.to_string_lossy().to_string()),
                        metainfo: Some(base64::engine::general_purpose::STANDARD.encode(content)),
                        paused: Some(u64::try_from(would_be_left).unwrap_or(0) < self.safe_space),
                        ..TorrentAddArgs::default()
                    },
                    would_be_left,
                    would_be_size,
                    exists,
                    &hsh,
                )?;
                std::fs::remove_file(path).context("remove_file")?;
            }
            TorrentLoc::Url(url) => {
                let magnet = MagnetURI::from_str(url.as_str())
                    .map_err(MagnetURIError)
                    .context("Non magnet urls not supported yet")?;
                if !magnet.is_strictly_valid() {
                    print_warn!(self.view.log(), "Not a strictly valid magnet link")?;
                }
                let mut hsh_tmp = magnet
                    .iter_topics()
                    .find_map(|h| match h {
                        magnet_uri::Topic::BitTorrentInfoHash(hs) => Some(hs),
                        _ => None,
                    })
                    .or_else(|| magnet.info_hash())
                    .ok_or_else(|| {
                        anyhow!("Magnet urls without any info hash are not supported")
                    })?;

                print_debug!(self.view.log(), "Magnet hsh before fixing: {}", &hsh_tmp)
                    .context("log")?;

                let mut hsh_owned;
                if hsh_tmp.len() != 40 {
                    let b32 = base32::decode(base32::Alphabet::Rfc4648 { padding: false }, hsh_tmp)
                        .ok_or_else(|| anyhow!("Invalid hash"))?;
                    hsh_owned = hex::encode(b32);
                    hsh_tmp = &hsh_owned;
                }
                hsh_owned = hsh_tmp.to_lowercase();

                let exists = self.check_existing(&hsh_owned)?;
                if let Some(time) = exists {
                    if !self
                        .view
                        .ask_existing(magnet.name().unwrap_or("magnet").as_bytes(), time)?
                    {
                        bail!(NothingToDo("Nothing to do"));
                    }
                }

                let (safe_space, total_size, download_dir) =
                    self.get_safe_space_and_dldir(dldir, &hsh_owned, use_existing)?;
                // about size as we don't know
                let would_be_left = safe_space - 5 * 1024 * 1024 * 1024;
                let would_be_size = total_size + 5 * 1024 * 1024 * 1024;
                self.add_torrent(
                    TorrentAddArgs {
                        paused: Some(u64::try_from(would_be_left).unwrap_or(0) < self.safe_space),
                        download_dir: Some(download_dir.to_string_lossy().to_string()),
                        filename: Some(url.as_str().to_string()),
                        ..TorrentAddArgs::default()
                    },
                    would_be_left,
                    would_be_size,
                    exists,
                    &hsh_owned,
                )?;
            }
        }
        Ok(())
    }

    fn add_torrent(
        &mut self,
        add_args: TorrentAddArgs,
        left: i64,
        total_size: i64,
        exists: Option<u64>,
        hsh: &str,
    ) -> Result<()> {
        let span = span!(Level::TRACE, "add_torrent");
        let _guard = span.enter();

        let full = add_args.paused.context("undefined paused")?;
        let response = self.client.torrent_add(add_args)?;
        // TODO don't insert if it was found in the db
        self.db.store(hsh)?;
        match &response {
            // TODO check hash returned matches above?
            TorrentAdded::TorrentAdded { .. } | TorrentAdded::TorrentDuplicate { .. } => {
                self.view.torrent_add_result(&TorrentAddResult {
                    response,
                    exists,
                    full,
                    left,
                    total_size,
                })
            }
        }
    }
}

// pub so that https://rust-embedded.github.io/book/design-patterns/hal/interoperability.html
#[derive(Debug)]
pub struct Trctl<T, C> {
    client: Client<T>,
    pub console: C,
    dldirs: Vec<PathBuf>,
    verify: bool,
    pub interactive: bool,
    pub dst_free_space_to_leave: u64,
    pub is_remote: bool,
}

#[derive(Debug)]
pub enum TorrentLoc {
    Path(PathBuf),
    Url(Url),
}

impl<T: TorrentCli, O: WriteColor, I: ReadLine> Trctl<T, Console<O, I>> {
    pub fn erase(&mut self, mut qcmd: QueryCmd, delete_data: bool) -> Result<()> {
        if qcmd.files {
            let torrents: Vec<Torrent> =
                self.client.torrent_get(None, None).context("torrent_get")?;
            let strs = qcmd.strs;
            qcmd.strs = Vec::with_capacity(1);
            for qst in strs {
                print_info!(self.console.log(), "{}:", qst).context("log")?;
                self.console.log().add_indent();
                qcmd.strs.push(qst);
                let filter = TorrentFilter::new(self.dldirs.as_slice(), &qcmd).context("filter")?;
                let filtered: Vec<&Torrent> = match filter.filter_torrents(torrents.iter()) {
                    Err(err) => {
                        if let Some(NoMatches) = err.downcast_ref::<NoMatches>() {
                            print_warn!(self.console.log(), "{}", err).context("log")?;
                            self.console.log().pop_indent();
                            continue;
                        }
                        return Err(err);
                    }
                    Ok(filtered_iter) => filtered_iter.collect(),
                };
                match Self::selectids(&mut self.console, &filtered, None, self.interactive) {
                    Ok(selected) => {
                        self.erase_selected(&selected, &torrents, delete_data)
                            .context("erase_selected")?;
                    }
                    Err(err) => {
                        if let Some(NothingToDo(msg)) = err.downcast_ref::<NothingToDo>() {
                            print_warn!(self.console.log(), "{}", msg).context("log")?;
                        }
                        return Err(err);
                    }
                }
                qcmd.strs.pop();
                self.console.log().pop_indent();
            }
        } else {
            let filtered: Vec<Torrent> = self
                .client
                .torrent_query_sort(None, &qcmd)
                .context("query")?;
            let selected = Self::selectids(&mut self.console, &filtered, None, self.interactive)
                .context("selectids")?;
            self.erase_selected(&selected, &filtered, delete_data)
                .context("erase_selected")?;
        }
        Ok(())
    }

    //fn select_one<TOR: Borrow<Torrent>>(
    //    &mut self,
    //    torrents: &[TOR]
    //) -> Result<usize> {
    //    let selected = self.console.confirm(torrents, Some(ConfirmAction::One))?;
    //    if selected.is_empty() {
    //        Err(anyhow!(NothingToDo("No selection")))
    //    } else {
    //        assert_eq!(selected.len(), 1);
    //        Ok(selected[0])
    //    }
    //}

    fn selectids<TOR: Borrow<Torrent>>(
        console: &mut Console<O, I>,
        torrents: &[TOR],
        mut action: Option<ConfirmAction>,
        interactive: bool,
    ) -> Result<Vec<usize>> {
        if !interactive {
            if action.is_none() {
                action = Some(ConfirmAction::All);
            } else if matches!(action, Some(ConfirmAction::One)) && torrents.len() > 1 {
                bail!("Too many matches ({})", torrents.len());
            }
        }
        let selected = console.confirm(torrents, action)?;
        if selected.is_empty() {
            Err(anyhow!(NothingToDo("No selection")))
        } else {
            Ok(selected)
        }
    }

    fn erase_selected(
        &mut self,
        selected: &[usize],
        torrents: &[Torrent],
        delete_data: bool,
    ) -> Result<()> {
        if selected.is_empty() {
            return Ok(());
        }

        let msg = if delete_data { "rm" } else { "erase" };
        for &i in selected {
            print_info!(
                self.console.log(),
                "{}: {}",
                msg,
                torrents[i].name.as_deref().unwrap_or("<unknown>")
            )?;
        }

        // ok to panic as it is ensured that the id exists
        let ids = selected
            .iter()
            .map(|&i| {
                Ok(torrents[i]
                    .hash_string
                    .as_ref()
                    .context("undefined hash")?
                    .clone())
            })
            .collect::<Result<Vec<String>>>()?;
        self.client.torrent_remove(ids, delete_data)?;

        if delete_data {
            let (_, mut errors): (Vec<_>, Vec<_>) = selected
                .iter()
                .map(|&i| -> Result<()> {
                    let it = &torrents[i];
                    let d = it
                        .download_dir
                        .as_ref()
                        .ok_or_else(|| anyhow!("no dldir"))?;
                    let h = it.hash_string.as_ref().ok_or_else(|| anyhow!("no hash"))?;
                    let p = std::path::Path::new(d);
                    if p.file_name() == Some(std::ffi::OsStr::new(h)) {
                        if self.is_remote {
                            print_info!(
                                self.console.log(),
                                "not removing the hash dir of a remote torrent {}",
                                d
                            )?;
                            return Ok(());
                        }
                        print_info!(self.console.log(), "rmdir {}", d)?;
                        std::fs::remove_dir(p).or_else(|e| {
                            print_error!(self.console.log(), "{}", e)?;
                            Err(e.into())
                        })
                    } else {
                        Ok(())
                    }
                })
                .partition(Result::is_ok);
            if let Some(Err(res)) = errors.pop() {
                return Err(res);
            }
        }

        Ok(())
    }

    //fn flatten<'n, X>(torrents: &'n[X], selected: &'n[usize]) -> impl Iterator<Item = &'n X>
    //{
    //    selected.iter().map(move |&i| &torrents[i])
    //}

    pub fn list_trackers(&mut self, qcmd: &QueryCmd) -> Result<()> {
        let torrents = self.client.torrent_query(None, qcmd)?;
        let mut trackers = <std::collections::HashMap<String, usize>>::new();
        for mut tor in torrents {
            if let Some(ts) = tor.trackers.take() {
                for t in ts {
                    *trackers.entry(t.announce).or_insert(0) += 1;
                }
            }
        }

        let mut count_vec: Vec<(&String, &usize)> = trackers.iter().collect();
        count_vec.sort_by(|a, b| b.1.cmp(a.1));
        for (t, count) in count_vec {
            writeln!(self.console.out(), "{count:4}: {t}")?;
        }
        Ok(())
    }

    pub fn query(&mut self, qcmd: &QueryCmd) -> Result<()> {
        let torrents = self.client.torrent_query_sort(None, qcmd)?;
        self.console.print_filtered(&torrents)
    }

    pub fn set_location(&mut self, qcmd: &QueryCmd, mv: bool, location: String) -> Result<()> {
        let torrents: Vec<Torrent> = self.client.torrent_query_sort(None, qcmd)?;
        let selected = Self::selectids(&mut self.console, &torrents, None, self.interactive)?;
        let ids = selected
            .iter()
            .map(|&i| {
                Ok(torrents[i]
                    .hash_string
                    .as_ref()
                    .context("undefined id")?
                    .clone())
            })
            .collect::<Result<Vec<String>>>()?;
        self.client.set_location(ids, mv, location)?;
        let selected_torrents: Vec<&Torrent> = selected.iter().map(|&i| &torrents[i]).collect();
        self.console
            .torrent_action_ok(selected_torrents, Action::SetLocation { moved: mv })?;
        Ok(())
    }

    pub fn action(&mut self, ori_qcmd: &QueryCmd, action: TorrentAction) -> Result<()> {
        let mut qcmd = ori_qcmd.clone();
        match action {
            TorrentAction::StartNow => {
                qcmd.status.extend_from_slice(&[
                    client::MyTorrentStatus::QueuedToDownload,
                    client::MyTorrentStatus::QueuedToSeed,
                    client::MyTorrentStatus::QueuedToVerify,
                    client::MyTorrentStatus::Stopped,
                ]);
                qcmd.finished = Some(false);
            }
            TorrentAction::Start => {
                qcmd.status.push(client::MyTorrentStatus::Stopped);
                qcmd.finished = Some(false);
            }
            TorrentAction::Stop => qcmd.status.extend_from_slice(&[
                client::MyTorrentStatus::QueuedToDownload,
                client::MyTorrentStatus::QueuedToSeed,
                client::MyTorrentStatus::QueuedToVerify,
                client::MyTorrentStatus::Seeding,
                client::MyTorrentStatus::Downloading,
            ]),
            _ => {}
        }
        let torrents: Vec<Torrent> = self.client.torrent_query_sort(None, &qcmd)?;
        let selected = Self::selectids(&mut self.console, &torrents, None, self.interactive)?;

        let ids = selected
            .iter()
            .map(|&i| {
                Ok(torrents[i]
                    .hash_string
                    .as_ref()
                    .context("undefined id")?
                    .clone())
            })
            .collect::<Result<Vec<String>>>()?; // TODO does this short circuit on err or are all the elements collected
                                                // first??
        self.client.torrent_action(ids.clone(), action)?;
        let selected_torrents: Vec<&Torrent> = selected.iter().map(|&i| &torrents[i]).collect();
        self.console
            .torrent_action_ok(selected_torrents, Action::TorrentAction(action))?;
        Ok(())
    }

    pub fn mv(
        &mut self,
        qcmd: &QueryCmd,
        destination: &Path,
        force: bool,
        verify: Option<bool>,
        config_path: &Path,
    ) -> Result<()> {
        if self.is_remote {
            bail!("Cannot mv files in a remote host");
        }

        let filtered: Vec<Torrent> = self.client.torrent_query_sort(None, qcmd)?;
        let selected = Self::selectids(&mut self.console, &filtered, None, self.interactive)?;

        let mut last_error = None;
        let mut errors = 0;
        for i in selected {
            let tor = &filtered[i];
            print_info!(
                self.console.log(),
                "mv {}",
                tor.name.as_deref().unwrap_or("missing")
            )?;

            let mut p = std::process::Command::new("/usr/lib/trctl/move.sh");
            p.env(
                "TR_FREE_SPACE_TO_LEAVE",
                format!("{}", self.dst_free_space_to_leave),
            )
            .env("TR_FORCE", if force { "1" } else { "0" })
            .env("TR_CONFIG_PATH", config_path)
            .env(
                "TR_VERIFY",
                if verify.unwrap_or(self.verify) {
                    "1"
                } else {
                    "0"
                },
            )
            .env("TR_TORRENT_ROOT", &self.console.base_dir)
            .env(
                "TR_TORRENT_FILE",
                tor.torrent_file
                    .as_ref()
                    .ok_or_else(|| anyhow!("torrent_file missing"))?,
            )
            .env(
                "TR_TORRENT_NAME",
                tor.name.as_ref().ok_or_else(|| anyhow!("name missing"))?,
            )
            .env(
                "TR_TORRENT_DIR",
                tor.download_dir
                    .as_ref()
                    .ok_or_else(|| anyhow!("download dir missing"))?,
            )
            .env(
                "TR_TORRENT_HASH",
                tor.hash_string
                    .as_ref()
                    .ok_or_else(|| anyhow!("torrent hash missing"))?,
            );
            p.env("TR_TORRENT_DESTINATION", destination);
            let status = p.status()?;
            if !status.success() {
                last_error = status.code();
                errors += 1;
                print_warn!(self.console.log(), "move: {:?}", status)?;
            }
        }
        if errors > 1 {
            bail!(Multiple(errors))
        }
        if let Some(3) = last_error {
            bail!(NotEnoughSpace)
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::MockRequest;
    use crate::config::Config;
    use crate::console::imps::tests::MockCon;
    use crate::console::DefLog;
    use pretty_assertions::assert_eq;

    #[test]
    #[ignore]
    #[allow(clippy::no_effect)]
    #[allow(path_statements)]
    fn visibility() {
        let log = DefLog::default();
        let builder = Config::get("tester");
        let mut trctl = builder
            .new_trctl_input(log, std::io::stdin())
            .map_err(|_| "error")
            .unwrap();
        trctl.query(&QueryCmd::default()).unwrap();
        trctl.query(&QueryCmd::default()).unwrap();
    }

    fn new_mock<F: FnOnce(&mut Trctl<MockRequest, MockCon>)>(f: F) {
        let builder = Config::get_mock();
        let log = builder.mock_log().unwrap();
        let mut trctl = builder.mock_trctl(log).unwrap();
        f(&mut trctl);
    }

    #[test]
    fn mock_query_none() {
        new_mock(|trctl| {
            trctl.client.imp.mock_data = vec![];
            if let Err(err) = trctl.query(&QueryCmd::default()) {
                if let Some(NoMatches) = err.downcast_ref::<NoMatches>() {
                } else {
                    panic!("should have nomatch");
                }
            } else {
                panic!("should have errorred");
            }
        });
    }

    #[test]
    #[should_panic(expected = "rpc request failed")]
    fn mock_fail_rpc() {
        new_mock(|trctl| {
            trctl.client.imp.fail_rpc = true;
            trctl.query(&QueryCmd::default()).expect("help");
        });
    }

    #[test]
    fn mock_query() {
        new_valid_mock(|trctl, qcmd| {
            log::set_max_level(log::LevelFilter::Info);
            trctl.query(&qcmd).unwrap();
            assert_eq!(
                trctl.console.log.to_string().unwrap(),
                "ID     Done     Have     Size       ETA       Up     Down  Ratio  Status     Name\n   \
                    1   100%     2.4G     2.4G   Unknown        0        0    0.8  Idle       dl//testing.pdf\n\
                 Sum:            2.4G                           0        0\n"
                 );
        });
    }

    fn new_valid_mock<F: FnOnce(&mut Trctl<MockRequest, MockCon>, QueryCmd)>(f: F) {
        new_mock(|trctl| {
            let mut qcmd = QueryCmd::default();
            qcmd.strs.push("testing.pdf".to_string());
            f(trctl, qcmd);
        });
    }

    #[test]
    #[should_panic(expected = "unexpected end of file")]
    fn mock_erase_no_selection() {
        new_valid_mock(|trctl, qcmd| {
            log::set_max_level(log::LevelFilter::Info);
            trctl.erase(qcmd, false).expect("ohno");
            //assert_eq!(trctl.console.log.io.to_string().expect("heww"), "hello");
        });
    }

    #[test]
    fn mock_erase() {
        new_valid_mock(|trctl, qcmd| {
            trctl.console.input.input = "y\n".to_string();
            log::set_max_level(log::LevelFilter::Info);
            trctl.erase(qcmd, false).unwrap();
            assert_eq!(trctl.console.log.to_string().unwrap(),
            "ID     Done     Have     Size       ETA       Up     Down  Ratio  Status     Name\n   \
                1   100%     2.4G     2.4G   Unknown        0        0    0.8  Idle       dl//testing.pdf\n\
             Sum:            2.4G                           0        0\n\
             Select [y/N]: -- erase: testing.pdf\n"
            );
            //"ID     Done     Have     Size       ETA       Up     Down  Ratio  Status     Name\n\
            //70   100%     2.4G     2.4G   Unknown        0        0    0.8  Idle       dl//testing.pdf\n\
        });
    }

    #[test]
    #[should_panic(expected = "Nothing found")]
    fn mock_erase_fail() {
        new_mock(|trctl| {
            let mut qcmd = QueryCmd::default();
            qcmd.strs.push("not found.pdf".to_string());
            trctl.erase(qcmd, false).unwrap();
        });
    }

    #[test]
    // TODO
    fn mock_erase_multiple_fail() {
        new_mock(|trctl| {
            let mut qcmd = QueryCmd::default();
            qcmd.strs.push("not found.pdf".to_string());
            qcmd.strs.push("not found2.pdf".to_string());
            qcmd.files = true;
            trctl.erase(qcmd, false).unwrap();
        });
    }

    #[test]
    fn mock_erase_invalid_input() {
        new_mock(|trctl| {
            let mut qcmd = QueryCmd::default();
            qcmd.strs.push("tes".to_string());
            trctl.console.input.input = "y\n6\n-2\n2\na\n".to_string();
            log::set_max_level(log::LevelFilter::Info);
            trctl.erase(qcmd, false).unwrap();
            assert_eq!(trctl.console.log.to_string().unwrap(),
            "ID     Done     Have     Size       ETA       Up     Down  Ratio  Status     Name\n   \
                1   100%     2.4G     2.4G   Unknown        0        0    0.8  Idle       dl//testing.pdf\n   \
                2   100%     2.4G     2.4G   Unknown        0        0    0.8  Idle       dl//testing2.pdf\n       \
                    error: error!!!\n   \
                3   100%     2.4G     2.4G   Unknown        0        0    0.8  Idle       dl//testing3.pdf\n\
             Sum:            7.1G                           0        0\n\
             Select [a/{n}/N]: Select [a/{n}/N]: Select [a/{n}/N]: Select [a/{n}/N]: -- erase: testing2.pdf\n\
             -w invalid digit found in string\n\
             -w Invalid id\n\
             -w Invalid id\n"
            );
        });
    }
}
