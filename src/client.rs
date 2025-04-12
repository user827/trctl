#[allow(unused_imports)]
use crate::config::{Builder, Config};
use crate::errors::*;
use clap::{Args, ValueEnum};
use regex::{Regex, RegexBuilder};
use std::borrow::Borrow;
use std::convert::TryFrom;
use std::ops::{Deref, DerefMut};
use std::path::PathBuf;
use tokio::runtime::Runtime;
use transmission_rpc::types::TorrentStatus;
use transmission_rpc::types::{ErrorType, Priority};
use transmission_rpc::types::{
    FreeSpace, RpcResponse, RpcResponseArgument, SessionGet, Torrent, TorrentAddedOrDuplicate,
};
pub use transmission_rpc::types::{Id, TorrentAction, TorrentAddArgs, TorrentGetField};
use transmission_rpc::TransClient;

#[derive(Debug)]
pub enum TorrentAdded {
    TorrentAdded {
        id: Option<i64>,
        hash_string: Option<String>,
        name: Option<String>,
    },
    TorrentDuplicate {
        id: Option<i64>,
        hash_string: Option<String>,
        name: Option<String>,
    },
}

impl TryFrom<TorrentAddedOrDuplicate> for TorrentAdded {
    type Error = Error;
    fn try_from(added: TorrentAddedOrDuplicate) -> Result<Self> {
        match added {
            TorrentAddedOrDuplicate::Error => bail!("TorrentAddedOrDuplicate error"),
            TorrentAddedOrDuplicate::TorrentAdded(Torrent {
                id,
                hash_string,
                name,
                ..
            }) => Ok(Self::TorrentAdded {
                id,
                hash_string,
                name,
            }),
            TorrentAddedOrDuplicate::TorrentDuplicate(Torrent {
                id,
                hash_string,
                name,
                ..
            }) => Ok(Self::TorrentDuplicate {
                id,
                hash_string,
                name,
            }),
        }
    }
}

#[derive(Args, Debug, Clone, Default)]
#[allow(clippy::struct_excessive_bools)]
pub struct QueryCmd {
    /// Case sensitive search. Is also enabled with uppercase in the query
    #[arg(long, short)]
    pub use_case: bool,
    /// Exact match on torrent or tracker name
    #[arg(long, short)]
    pub exact: bool,
    /// Match finished
    #[arg(long)]
    pub finished: Option<bool>,
    /// Match torrents with an error
    #[arg(long)]
    pub error: bool,
    /// Match completed
    #[arg(long)]
    pub complete: bool,
    /// Match incomplete
    #[arg(long)]
    pub incomplete: bool,
    /// Match completed that are still in dldir
    #[arg(long)]
    pub move_aborted: bool,
    /// Match files not in dldir
    #[arg(long)]
    pub moved: bool,
    /// Match moved and finished torrents
    #[arg(long)]
    pub cleanable: bool,
    /// All the strings have to match instead of one
    #[arg(long)]
    pub and: bool,
    /// Exact match on torrent name
    #[arg(long)]
    pub files: bool,
    /// Sort the output
    #[arg(long, short)]
    pub sort: Option<Sort>,
    /// Print in reverse
    #[arg(long, short)]
    pub reverse: bool,
    /// Match ids
    #[arg(long)]
    pub ids: Vec<i64>,
    /// Match hashes
    #[arg(long)]
    pub hsh: Vec<String>,
    /// Match trackers
    #[arg(long)]
    pub trackers: Vec<String>,
    /// Match status(es)
    #[arg(long)]
    pub status: Vec<MyTorrentStatus>,
    /// Query names
    pub strs: Vec<String>,
}

#[derive(Debug)]
pub struct Client<T> {
    pub imp: T,
    pub dldirs: Vec<PathBuf>,
}

impl<T: TorrentCli> Deref for Client<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.imp
    }
}

impl<T: TorrentCli> DerefMut for Client<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.imp
    }
}

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq)]
pub enum MyTorrentStatus {
    Downloading,
    QueuedToDownload,
    QueuedToSeed,
    QueuedToVerify,
    Seeding,
    Stopped,
    Verifying,
}

impl From<TorrentStatus> for MyTorrentStatus {
    fn from(value: TorrentStatus) -> Self {
        match value {
            TorrentStatus::Downloading => Self::Downloading,
            TorrentStatus::QueuedToDownload => Self::QueuedToDownload,
            TorrentStatus::QueuedToSeed => Self::QueuedToSeed,
            TorrentStatus::QueuedToVerify => Self::QueuedToVerify,
            TorrentStatus::Seeding => Self::Seeding,
            TorrentStatus::Stopped => Self::Stopped,
            TorrentStatus::Verifying => Self::Verifying,
        }
    }
}

#[derive(ValueEnum, Debug, Clone, Copy)]
pub enum Sort {
    Id,
    Name,
    Urate,
    Drate,
    Size,
}

impl Default for Sort {
    fn default() -> Self {
        Self::Id
    }
}

impl<T: TorrentCli> Client<T> {
    fn sort_maybe_reverse<K: Ord, F>(torrents: &mut [Torrent], mut f: F, reverse: bool)
    where
        F: FnMut(&Torrent) -> K,
    {
        if reverse {
            torrents.sort_unstable_by_key(|x| std::cmp::Reverse(f(x)));
        } else {
            torrents.sort_unstable_by_key(f);
        }
    }

    // TODO
    //fn merge_fields(mut fields: Vec<TorrentGetField>, qcmd: &QueryCmd) -> Vec<TorrentGetField> {
    //    if let Some(true) = qcmd.finished {
    //        fields.push(TorrentGetField::IsFinished);
    //    }
    //    fields
    //}

    pub fn sort(torrents: &mut [Torrent], sort: Sort, reverse: bool) {
        match sort {
            Sort::Id => {
                if reverse {
                    torrents.sort_unstable_by(|x, y| y.id.cmp(&x.id));
                } else {
                    torrents.sort_unstable_by(|x, y| x.id.cmp(&y.id));
                }
            }
            Sort::Name => {
                if reverse {
                    torrents.sort_unstable_by(|x, y| y.name.cmp(&x.name));
                } else {
                    torrents.sort_unstable_by(|x, y| x.name.cmp(&y.name));
                }
            }
            Sort::Urate => Self::sort_maybe_reverse(torrents, |x| x.rate_upload, reverse),
            Sort::Drate => Self::sort_maybe_reverse(torrents, |x| x.rate_download, reverse),
            Sort::Size => Self::sort_maybe_reverse(torrents, |x| x.size_when_done, reverse),
        }
    }

    // Note that filtering might not work if correct fields are not selected
    pub fn torrent_query_sort(
        &mut self,
        fields: Option<Vec<TorrentGetField>>,
        qcmd: &QueryCmd,
    ) -> Result<Vec<Torrent>> {
        let torrents = self.do_torrent_query(fields, qcmd)?;
        let filter = TorrentFilter::new(self.dldirs.as_slice(), qcmd)?;
        let mut filtered: Vec<Torrent> = filter.filter_torrents(torrents)?.collect();

        Self::sort(&mut filtered, qcmd.sort.unwrap_or_default(), qcmd.reverse);
        Ok(filtered)
    }

    fn do_torrent_query(
        &mut self,
        fields: Option<Vec<TorrentGetField>>,
        qcmd: &QueryCmd,
    ) -> Result<Vec<Torrent>> {
        //let fields = fields.map(|fs| Self::merge_fields(fs, qcmd));
        let ids = if qcmd.ids.is_empty() && qcmd.hsh.is_empty() {
            None
        } else {
            let mut v: Vec<Id> = qcmd.ids.iter().map(|&i| Id::Id(i)).collect();
            v.extend(qcmd.hsh.iter().map(|i| Id::Hash(i.to_string())));
            Some(v)
        };
        self.torrent_get(fields, ids)
    }

    pub fn torrent_query<'x>(
        &'x mut self,
        fields: Option<Vec<TorrentGetField>>,
        qcmd: &'x QueryCmd,
    ) -> Result<impl Iterator<Item = Torrent> + 'x> {
        let torrents = self.do_torrent_query(fields, qcmd)?;
        let filter = TorrentFilter::new(self.dldirs.as_slice(), qcmd)?;
        filter.filter_torrents(torrents)
    }

    pub fn torrent_get(
        &mut self,
        fields: Option<Vec<TorrentGetField>>,
        ids: Option<Vec<Id>>,
    ) -> Result<Vec<Torrent>> {
        self.imp.torrent_get(fields, ids)
    }
}

pub trait TorrentCli {
    fn torrent_add(&mut self, args: TorrentAddArgs) -> Result<TorrentAdded>;

    fn free_space(&mut self, path: String) -> Result<FreeSpace>;

    fn session_get(&mut self) -> Result<SessionGet>;

    fn torrent_get(
        &mut self,
        fields: Option<Vec<TorrentGetField>>,
        ids: Option<Vec<Id>>,
    ) -> Result<Vec<Torrent>>;

    fn torrent_remove(&mut self, ids: Vec<String>, delete_local_data: bool) -> Result<()>;

    fn torrent_action(&mut self, ids: Vec<String>, action: TorrentAction) -> Result<()>;

    fn set_location(&mut self, ids: Vec<String>, mv: bool, location: String) -> Result<()>;
}

#[derive(Debug)]
pub struct TorrentFilter<'a> {
    pub dldirs: &'a [PathBuf],
    pub trackers: Vec<Regex>,
    pub strs: Vec<Regex>,
    pub qcmd: &'a QueryCmd,
}

impl<'x> TorrentFilter<'x> {
    pub fn new(dldirs: &'x [PathBuf], qcmd: &'x QueryCmd) -> Result<Self> {
        let strs = qcmd
            .strs
            .iter()
            .map(|s| {
                let mut builder = if qcmd.exact || qcmd.files {
                    RegexBuilder::new(&format!("^{}$", regex::escape(s)))
                } else {
                    RegexBuilder::new(&regex::escape(s))
                };

                if !qcmd.use_case && !qcmd.files && !s.chars().any(char::is_uppercase) {
                    builder.case_insensitive(true);
                }

                builder.build().context("regex build failed")
            })
            .collect::<Result<Vec<regex::Regex>>>()?;

        let trackers = qcmd
            .trackers
            .iter()
            .map(|s| {
                let mut builder = if qcmd.exact {
                    RegexBuilder::new(&format!("^{}$", regex::escape(s)))
                } else {
                    RegexBuilder::new(&regex::escape(s))
                };

                if !qcmd.use_case && !s.chars().any(char::is_uppercase) {
                    builder.case_insensitive(true);
                }

                builder.build().context("regex build failed")
            })
            .collect::<Result<Vec<regex::Regex>>>()?;

        Ok(Self {
            dldirs,
            trackers,
            strs,
            qcmd,
        })
    }

    pub fn filter_torrents<I>(self, torrents: I) -> Result<impl Iterator<Item = I::Item> + 'x>
    where
        I: IntoIterator + 'x,
        I::Item: Borrow<Torrent>,
    {
        let mut iter = torrents
            .into_iter()
            .filter(move |torrent| self.torrent_filter(torrent.borrow()).unwrap_or(false))
            .peekable();
        if iter.peek().is_none() {
            bail!(NoMatches);
        }
        Ok(iter)
    }

    fn torrent_filter(&self, tor: &Torrent) -> Option<bool> {
        if !self.strs.is_empty() {
            if self.qcmd.and {
                for qstr in &self.strs {
                    let name = tor.name.as_ref()?;
                    if !qstr.is_match(name) {
                        return Some(false);
                    }
                }
            } else {
                let mut found = false;
                let name = tor.name.as_ref()?;
                for qstr in &self.strs {
                    if qstr.is_match(name) {
                        found = true;
                        break;
                    }
                }
                if !found {
                    return Some(false);
                }
            }
        }

        if !self.trackers.is_empty() {
            let mut found = false;
            if self.qcmd.exact {
                for tqs in &self.trackers {
                    for tracker in tor.trackers.as_ref()? {
                        if tqs.is_match(&tracker.announce) {
                            found = true;
                            break;
                        }
                    }
                    if found {
                        break;
                    }
                }
            } else {
                for tracker in tor.trackers.as_ref()? {
                    found = true;
                    for tqs in &self.trackers {
                        if !tqs.is_match(&tracker.announce) {
                            found = false;
                            break;
                        }
                    }
                    if found {
                        break;
                    }
                }
            }
            if !found {
                return Some(false);
            }
        }

        {
            if !self.qcmd.status.is_empty() {
                let mut matches = false;
                for status in &self.qcmd.status {
                    if MyTorrentStatus::from(tor.status?) == *status {
                        matches = true;
                        break;
                    }
                }
                if !matches {
                    return Some(false);
                }
            }
        }

        if let Some(finished) = self.qcmd.finished {
            if finished != tor.is_finished? {
                return Some(false);
            }
        }

        if self.qcmd.complete && (tor.left_until_done? != 0 || tor.size_when_done? == 0) {
            return Some(false);
        }

        if self.qcmd.incomplete && (tor.left_until_done? == 0 && tor.size_when_done? != 0) {
            return Some(false);
        }

        if self.qcmd.error && tor.error? == ErrorType::Ok {
            return Some(false);
        }

        // TODO have to check status too?
        if self.qcmd.move_aborted
            && !(tor.left_until_done? == 0 && self.in_dl_dir(tor)? && tor.size_when_done? != 0)
        {
            // SizeWhenDone to catch not loaded magnets
            return Some(false);
        }

        if self.qcmd.moved
            && !(tor.left_until_done? == 0 && !self.in_dl_dir(tor)? && tor.size_when_done? != 0)
        {
            // SizeWhenDone to catch not loaded magnets
            return Some(false);
        }

        if self.qcmd.cleanable && !self.filter_is_cleanable(tor)? {
            return Some(false);
        }

        Some(true)
    }

    fn filter_is_cleanable(&self, tor: &Torrent) -> Option<bool> {
        Some(
            tor.is_finished?
                && !self.in_dl_dir(tor)?
                && !matches!(tor.status?, TorrentStatus::QueuedToVerify)
                && !matches!(tor.status?, TorrentStatus::Verifying),
        )
    }

    fn in_dl_dir(&self, tor: &Torrent) -> Option<bool> {
        let p = std::path::Path::new(tor.download_dir.as_ref()?);
        Some(self.dldirs.iter().any(|d| p.starts_with(d)))
    }
}

pub struct SyncRequest {
    pub client: TransClient,
    pub tokio: Runtime,
}

fn call<RS, C>(tokio: &Runtime, f: C) -> Result<RS>
where
    C: std::future::Future<Output = transmission_rpc::types::Result<RpcResponse<RS>>>,
    RS: RpcResponseArgument,
{
    tokio.block_on(async {
        let res = f.await.map_err(|e| anyhow!("rpc call: {:#}", e))?;
        if !res.is_ok() {
            bail!("rpc request failed with: '{}'", res.result);
        }
        //if let OrError::Success(args) = res.arguments {
        //    return Ok(args);
        //}
        //bail!("Error but res.is_ok()");
        Ok(res.arguments)
    })
}

impl TorrentCli for SyncRequest {
    fn torrent_add(&mut self, args: TorrentAddArgs) -> Result<TorrentAdded> {
        TorrentAdded::try_from(call(&self.tokio, self.client.torrent_add(args))?)
    }

    fn session_get(&mut self) -> Result<SessionGet> {
        call(&self.tokio, self.client.session_get())
    }

    fn free_space(&mut self, path: String) -> Result<FreeSpace> {
        call(&self.tokio, self.client.free_space(path))
    }

    fn torrent_remove(&mut self, ids: Vec<String>, delete_local_data: bool) -> Result<()> {
        call(
            &self.tokio,
            self.client
                .torrent_remove(ids.into_iter().map(Id::Hash).collect(), delete_local_data),
        )?;
        Ok(())
    }

    fn torrent_action(&mut self, ids: Vec<String>, action: TorrentAction) -> Result<()> {
        call(
            &self.tokio,
            self.client
                .torrent_action(action, ids.into_iter().map(Id::Hash).collect()),
        )?;
        Ok(())
    }

    fn set_location(&mut self, ids: Vec<String>, mv: bool, location: String) -> Result<()> {
        call(
            &self.tokio,
            self.client.torrent_set_location(
                ids.into_iter().map(Id::Hash).collect(),
                location,
                Some(mv),
            ),
        )?;
        Ok(())
    }

    fn torrent_get(
        &mut self,
        fields: Option<Vec<TorrentGetField>>,
        oids: Option<Vec<Id>>,
    ) -> Result<Vec<Torrent>> {
        Ok(call(&self.tokio, self.client.torrent_get(fields, oids))?.torrents)
    }
}

pub struct MockRequest {
    pub mock_data: Vec<Torrent>,
    pub fail_rpc: bool,
}

impl Default for MockRequest {
    fn default() -> MockRequest {
        MockRequest {
            mock_data: [
                test_torrent(1, "testing.pdf"),
                Torrent {
                    error_string: Some("error!!!".into()),
                    ..test_torrent(2, "testing2.pdf")
                },
                test_torrent(3, "testing3.pdf"),
            ]
            .to_vec(),
            fail_rpc: false,
        }
    }
}

impl TorrentCli for MockRequest {
    fn torrent_action(&mut self, _ids: Vec<String>, _action: TorrentAction) -> Result<()> {
        Ok(())
    }

    fn torrent_add(&mut self, _args: TorrentAddArgs) -> Result<TorrentAdded> {
        Ok(TorrentAdded::TorrentAdded {
            id: Some(6),
            name: Some("added.pdf".into()),
            hash_string: Some("03a4f88adee883a3a135f10042442894af4167f7".into()),
        })
    }

    fn session_get(&mut self) -> Result<SessionGet> {
        Ok(SessionGet {
            blocklist_enabled: false,
            download_dir: "/mydldir".to_string(),
            encryption: "wut?".to_string(),
            rpc_version: 7,
            rpc_version_minimum: 3,
            version: "2.0".to_string(),
        })
    }

    fn free_space(&mut self, path: String) -> Result<FreeSpace> {
        Ok(FreeSpace {
            path,
            size_bytes: 50 * 1024 * 1024 * 1024,
        })
    }

    fn torrent_get(
        &mut self,
        _fields: Option<Vec<TorrentGetField>>,
        _ids: Option<Vec<Id>>,
    ) -> Result<Vec<Torrent>> {
        if self.fail_rpc {
            bail!("rpc request failed");
        }
        Ok(self.mock_data.clone())
    }

    fn torrent_remove(&mut self, _ids: Vec<String>, _delete_local_data: bool) -> Result<()> {
        if self.fail_rpc {
            bail!("rpc request failed");
        }
        Ok(())
    }

    fn set_location(&mut self, _ids: Vec<String>, _mv: bool, _location: String) -> Result<()> {
        Ok(())
    }
}

#[must_use]
#[allow(clippy::unreadable_literal)]
pub fn test_torrent<I: Into<String>>(id: i64, name: I) -> Torrent {
    let cfg = Config::default();
    Torrent {
        torrent_file: None,
        bandwidth_priority: Some(Priority::Low),
        file_count: Some(0),
        tracker_list: Some(String::new()),
        tracker_stats: Some(vec![]),
        seconds_seeding: Some(0),
        labels: None,
        is_private: Some(false),
        edit_date: Some(1604022244),
        activity_date: Some(1604022244),
        added_date: Some(1604022244),
        done_date: None,
        download_dir: Some(format!(
            "{}/abed48adeb5e396f54a7089cbe6c1f2bc1b0dbc8",
            cfg.dldirs[0].to_string_lossy()
        )),
        error: Some(ErrorType::Ok),
        error_string: Some(String::new()),
        eta: Some(-2),
        id: Some(id),
        is_finished: Some(false),
        is_stalled: Some(true),
        left_until_done: Some(0),
        metadata_percent_complete: Some(1.0),
        name: Some(name.into()),
        hash_string: Some("abed48adeb5e396f54a7089cbe6c1f2bc1b0dbc8".to_string()),
        peers_connected: Some(0),
        peers_getting_from_us: Some(0),
        peers_sending_to_us: Some(0),
        percent_done: Some(1.0),
        rate_download: Some(0),
        rate_upload: Some(0),
        recheck_progress: Some(0.0),
        seed_ratio_limit: Some(2.0),
        size_when_done: Some(2541190084),
        status: Some(TorrentStatus::Downloading),
        total_size: Some(2541190084),
        trackers: None,
        //trackers: Some([Trackers {
        //    id: 0,
        //    announce: "http://mysite.com/ann?arg=jees",
        //}]),
        upload_ratio: Some(0.8031),
        uploaded_ever: Some(2065294862),
        files: None,
        // for each file in files, whether or not they will be downloaded (0 or 1)
        wanted: None,
        // for each file in files, their download priority (low:-1,normal:0,high:1)
        priorities: None,
        file_stats: None,
    }
}

#[must_use]
pub fn new_torrent() -> Torrent {
    Torrent {
        torrent_file: None,
        bandwidth_priority: None,
        file_count: None,
        tracker_list: None,
        tracker_stats: None,
        seconds_seeding: None,
        labels: None,
        is_private: None,
        edit_date: None,
        activity_date: None,
        added_date: None,
        done_date: None,
        download_dir: None,
        error: None,
        error_string: None,
        eta: None,
        id: None,
        is_finished: None,
        is_stalled: None,
        left_until_done: None,
        metadata_percent_complete: None,
        name: None,
        hash_string: None,
        peers_connected: None,
        peers_getting_from_us: None,
        peers_sending_to_us: None,
        percent_done: None,
        rate_download: None,
        rate_upload: None,
        recheck_progress: None,
        seed_ratio_limit: None,
        size_when_done: None,
        status: None,
        total_size: None,
        trackers: None,
        upload_ratio: None,
        uploaded_ever: None,
        files: None,
        // for each file in files, whether or not they will be downloaded (0 or 1)
        wanted: None,
        // for each file in files, their download priority (low:-1,normal:0,high:1)
        priorities: None,
        file_stats: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn filter_cleanable() {
        let mut tor = new_torrent();
        let builder = Config::get("tester");
        let qcmd = QueryCmd::default();
        let filter = builder.new_filter(&qcmd).unwrap();
        assert_ne!(filter.filter_is_cleanable(&tor), Some(true));
        tor.is_finished = Some(true);
        tor.download_dir = Some(builder.cfg.dldirs[0].to_string_lossy().into());
        tor.status = Some(TorrentStatus::QueuedToVerify);
        assert_ne!(filter.filter_is_cleanable(&tor), Some(true));
        tor.status = Some(TorrentStatus::Stopped);
        tor.download_dir = None;
        assert_ne!(filter.filter_is_cleanable(&tor), Some(true));
        tor.download_dir = Some("/hello".to_string());
        assert_eq!(filter.filter_is_cleanable(&tor), Some(true));
    }

    #[test]
    fn filter_nones() {
        let builder = Config::get("tester");
        let tor = new_torrent();
        let mut qcmd = QueryCmd::default();

        let filter = builder.new_filter(&qcmd).unwrap();
        assert_eq!(filter.torrent_filter(&tor), Some(true));

        qcmd.files = true;
        let filter = builder.new_filter(&qcmd).unwrap();
        assert_eq!(filter.torrent_filter(&tor), Some(true));
        qcmd.files = false;

        qcmd.move_aborted = true;
        let filter = builder.new_filter(&qcmd).unwrap();
        assert_ne!(filter.torrent_filter(&tor), Some(true));
        qcmd.move_aborted = false;

        qcmd.cleanable = true;
        let filter = builder.new_filter(&qcmd).unwrap();
        assert_ne!(filter.torrent_filter(&tor), Some(true));
        qcmd.cleanable = false;
    }
}
