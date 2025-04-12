use std::fmt;
use std::path::Path;
use transmission_rpc::types::{Torrent as TrTorrent, TorrentStatus};

pub struct Torrent<'a> {
    pub torrent: &'a TrTorrent,
    pub base_dir: &'a Path,
}

impl<'a> Torrent<'a> {
    #[must_use]
    pub const fn get_header() -> &'static str {
        //"{:4}   {:>4}  {:>7}  {:>7}  {:>8}  {:>7}  {:>7}  {:5}  {:9}  Name",
        "ID     Done     Have     Size       ETA       Up     Down  Ratio  Status     Name"
    }

    #[must_use]
    pub fn percent_done(&self) -> impl fmt::Display {
        Maybe(self.torrent.percent_done.map(|n| n * 100.0), true)
    }

    #[must_use]
    pub fn downloaded_size(&self) -> impl fmt::Display {
        let downloaded_size = self.torrent.size_when_done.and_then(|x| {
            if x < 0 {
                None
            } else {
                self.torrent
                    .left_until_done
                    .and_then(|z| if z < 0 { None } else { Some(z) })
                    .map(|y| x - y)
            }
        });
        Maybe(downloaded_size.map(ByteSize), true)
    }

    #[must_use]
    pub fn error_mark(&self) -> impl fmt::Display {
        if self.torrent.error == Some(transmission_rpc::types::ErrorType::Ok) {
            ' '
        } else {
            '*'
        }
    }

    #[must_use]
    pub fn download_dir(&'a self) -> impl fmt::Display + 'a {
        DlDir(self)
    }

    #[must_use]
    pub fn id(&self) -> impl fmt::Display {
        Maybe(self.torrent.id, true)
    }
}

struct DlDir<'a>(&'a Torrent<'a>);

impl fmt::Display for DlDir<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        let torrent = self.0.torrent;
        match torrent.download_dir {
            None => write!(formatter, "{}", Maybe(None::<String>, false)),
            Some(ref dldir) => {
                let mut path = Path::new(dldir);
                path = path.strip_prefix(self.0.base_dir).unwrap_or(path);

                match torrent.hash_string {
                    None => write!(formatter, "{}", path.display()),
                    Some(ref hsh) => {
                        if path.ends_with(hsh) {
                            if let Some(parent) = path.parent() {
                                write!(formatter, "{}/", parent.display())?;
                            }
                        } else {
                            write!(formatter, "{}", path.display())?;
                        }
                        Ok(())
                    }
                }
            }
        }
    }
}

impl<'a> AsRef<Torrent<'a>> for Torrent<'a> {
    fn as_ref(&self) -> &Torrent<'a> {
        self
    }
}

impl fmt::Display for Torrent<'_> {
    /// Prints torrents
    ///
    /// # Example
    /// ```
    /// use trctl::config::Config;
    /// use trctl::display::Torrent;
    /// let config = Config::default();
    /// let mut tor = trctl::client::new_torrent();
    /// assert_eq!(
    ///     format!(
    ///         "{}",
    ///         Torrent { torrent: &tor, base_dir: &config.base_dir },
    ///     ),
    ///     "  NA*   NA%       NA       NA        NA       NA       NA     NA  NA         NA/NA"
    /// );
    /// let tor2 = trctl::client::test_torrent(70, "testing.pdf");
    /// assert_eq!(
    ///     format!("{}", Torrent { torrent: &tor2, base_dir: &config.base_dir }),
    ///     "  70   100%     2.4G     2.4G   Unknown        0        0    0.8  Idle       dl//testing.pdf"
    ///     );
    ///  ```
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        let Torrent { torrent, .. } = self;
        write!(
            formatter,
            "{:4}{}  {:3.0}%  {:7.1}  {:7.1}  {:>8}  {:7.1}  {:7.1}  {:5.1}  {:9}  {}/{}",
            self.id(),
            self.error_mark(),
            self.percent_done(),
            self.downloaded_size(),
            Maybe(torrent.size_when_done.map(ByteSize), true),
            Maybe(
                torrent.eta.map(|e| Eta {
                    eta: e,
                    left_until_done: torrent.left_until_done
                }),
                true
            ),
            Maybe(torrent.rate_upload.map(ByteSize), true),
            Maybe(torrent.rate_download.map(ByteSize), true),
            Maybe(torrent.upload_ratio, true),
            Maybe(Status::from_torrent(torrent), false),
            self.download_dir(),
            Maybe(torrent.name.as_ref(), false),
        )?;

        match torrent.error_string {
            Some(ref s) if !s.is_empty() => write!(formatter, "\n       error: {s}")?,
            _ => (),
        }

        Ok(())
    }
}

pub struct Maybe<T>(pub Option<T>, pub bool);

#[allow(dead_code)]
// because this is so nice
impl<T> Maybe<T> {
    #[must_use]
    pub fn new(is_numeric: bool) -> fn(Option<T>) -> Maybe<T> {
        if is_numeric {
            fn new<U>(option: Option<U>) -> Maybe<U> {
                Maybe(option, true)
            }
            new
        } else {
            fn new<U>(option: Option<U>) -> Maybe<U> {
                Maybe(option, false)
            }
            new
        }
    }
}

/// Maybe display some
/// ```
/// use trctl::display::Maybe;
/// assert_eq!(format!("{}", Maybe::<i64>(None, true)), "NA");
/// assert_eq!(format!("{:1}", Maybe::<i64>(None, true)), "NA");
/// assert_eq!(format!("{:4}", Maybe::<i64>(None, true)), "  NA");
/// assert_eq!(format!("{}", Maybe::<String>(None, false)), "NA");
/// assert_eq!(format!("{:1}", Maybe::<String>(None, false)), "NA");
/// assert_eq!(format!("{:4}", Maybe::<String>(None, false)), "NA  ");
/// assert_eq!(format!("{}", Maybe(Some(3), true)), "3");
/// assert_eq!(format!("{:1}", Maybe(Some(3), true)), "3");
/// assert_eq!(format!("{:4}", Maybe(Some(3), true)), "   3");
/// ```
impl<T: fmt::Display> fmt::Display for Maybe<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        match &self {
            // TODO other flags?
            Maybe(None, true) => write!(
                formatter,
                "{:>width$}",
                "NA",
                width = formatter.width().unwrap_or(0)
            ),
            Maybe(None, false) => write!(
                formatter,
                "{:width$}",
                "NA",
                width = formatter.width().unwrap_or(0)
            ),
            Maybe(Some(v), _) => v.fmt(formatter),
        }
    }
}

pub struct Status {
    status: TorrentStatus,
    is_finished: Option<bool>,
    recheck_progress: Option<f32>,
    peers_getting_from_us: Option<i64>,
    peers_sending_to_us: Option<i64>,
    left_until_done: Option<i64>,
}

impl Status {
    #[must_use]
    pub fn from_torrent(torrent: &TrTorrent) -> Option<Status> {
        Some(Status {
            status: torrent.status?,
            is_finished: torrent.is_finished,
            recheck_progress: torrent.recheck_progress,
            peers_getting_from_us: torrent.peers_getting_from_us,
            peers_sending_to_us: torrent.peers_sending_to_us,
            left_until_done: torrent.left_until_done,
        })
    }
}

impl fmt::Display for Status {
    /// Human readable status
    /// ```
    /// use trctl::display::Status;
    /// ```
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        let &Status {
            status,
            is_finished,
            recheck_progress,
            peers_getting_from_us,
            peers_sending_to_us,
            left_until_done,
        } = self;
        let width = formatter.width().unwrap_or(0);
        match status {
            TorrentStatus::Stopped => write!(
                formatter,
                "{:width$}",
                match is_finished {
                    None => "NA",
                    Some(true) => "Finished",
                    Some(false) => "Stopped",
                },
                width = width
            ),
            n @ (TorrentStatus::QueuedToVerify | TorrentStatus::Verifying) => write!(
                formatter,
                "{:width$} ({:3.0}%)",
                if n == TorrentStatus::QueuedToVerify {
                    "Will Verify"
                } else {
                    "Verifying"
                },
                Maybe(recheck_progress.map(|n| n * 100.0), true),
                width = if width < 7 { 0 } else { width - 7 }
            ),
            TorrentStatus::QueuedToDownload => {
                write!(formatter, "{:width$}", "Queued", width = width)
            }
            TorrentStatus::Downloading | TorrentStatus::Seeding => write!(
                formatter,
                "{:width$}",
                match (peers_getting_from_us, peers_sending_to_us) {
                    (None, _) | (_, None) => "ERROR",
                    (Some(x), Some(y)) if x != 0 && y != 0 => "Up & Down",
                    (_, Some(y)) if y != 0 => "Downloading",
                    (Some(x), _) if x != 0 => match left_until_done {
                        None => "ERROR",
                        Some(x) if x > 0 => "Uploading",
                        Some(_) => "Seeding",
                    },
                    _ => "Idle",
                },
                width = width
            ),
            TorrentStatus::QueuedToSeed => {
                write!(formatter, "{:width$}", "Queued Sd", width = width)
            }
        }
    }
}

pub struct Eta {
    pub eta: i64,
    pub left_until_done: Option<i64>,
}

/// Human readable duration
/// ```
/// use trctl::display::Eta;
/// assert_eq!(Eta { eta: 20, left_until_done: None }.to_string(), "20 sec");
/// assert_eq!(Eta { eta: 60, left_until_done: None }.to_string(), "1 min");
/// assert_eq!(Eta { eta: 61, left_until_done: None }.to_string(), "1 min");
/// assert_eq!(format!("{:7}", Eta { eta: 61, left_until_done: None }), "  1 min");
/// ```
impl fmt::Display for Eta {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        let &Eta {
            eta,
            left_until_done,
        } = self;
        let width = formatter.width().unwrap_or(0);
        if eta >= 0 {
            if eta < 60 {
                write!(
                    formatter,
                    "{:width$} sec",
                    eta,
                    width = if width < 4 { 0 } else { width - 4 }
                )
            } else if eta < (60 * 60) {
                write!(
                    formatter,
                    "{:width$} min",
                    eta / 60,
                    width = if width < 4 { 0 } else { width - 4 }
                )
            } else if eta < (60 * 60 * 24) {
                write!(
                    formatter,
                    "{:width$} hrs",
                    eta / (60 * 60),
                    width = if width < 4 { 0 } else { width - 4 }
                )
            } else {
                write!(
                    formatter,
                    "{:width$} days",
                    eta / (60 * 60 * 24),
                    width = if width < 5 { 0 } else { width - 5 }
                )
            }
        } else {
            write!(
                formatter,
                "{:>width$}",
                if eta == -2 {
                    "Unknown"
                } else if eta == -1 {
                    if left_until_done == Some(0) {
                        "Done"
                    } else {
                        "NA"
                    }
                } else {
                    "Err"
                },
                width = width
            )
        }
    }
}

pub struct ByteSize<T>(pub T);

impl<T> ByteSize<T> {
    fn fmt_float(bytes: f64, formatter: &mut fmt::Formatter) -> fmt::Result {
        let width = formatter.width().unwrap_or(0);
        let precision = formatter.precision().unwrap_or(1);
        let mut num = bytes / 1024.0;
        for unit in &["K", "M", "G", "T", "P", "E", "Z"] {
            if num.abs() < 1024.0 {
                return write!(
                    formatter,
                    "{:width$.*}{}",
                    precision,
                    num,
                    unit,
                    width = if width < 1 { 0 } else { width - 1 }
                );
            }
            num /= 1024.0;
        }
        write!(
            formatter,
            "{:width$.*} YiB",
            precision,
            num,
            width = if width < 4 { 0 } else { width - 4 }
        )
    }
}

impl fmt::Display for ByteSize<i64> {
    //fn to_kibibytes(bytes: i64) -> (i64, i64) {
    //((bytes + (1<<9) >> 10, 1<<8))
    //}

    /// Human readable bytes in si units.
    ///
    /// ```
    /// use trctl::display::ByteSize;
    /// assert_eq!(format!("{}", ByteSize(0i64)), "0");
    /// assert_eq!(format!("{}", ByteSize(1023i64)), "1023");
    /// assert_eq!(format!("{}", ByteSize(-1023i64)), "-1023");
    /// assert_eq!(format!("{}", ByteSize(1024i64)), "1.0K");
    /// assert_eq!(format!("{}", ByteSize(-1024i64)), "-1.0K");
    /// assert_eq!(format!("{:5}", ByteSize(1025i64)), " 1.0K");
    /// assert_eq!(ByteSize(2047i64).to_string(), "2.0K");
    /// assert_eq!(format!("{:6.2}", ByteSize(2100i64)), " 2.05K");
    /// assert_eq!(format!("{:.0}", ByteSize(2100i64)), "2K");
    /// ```
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        let bytes = self.0;
        let width = formatter.width().unwrap_or(0);
        if bytes.abs() < 1024 {
            return write!(formatter, "{bytes:width$}");
        }
        #[allow(clippy::cast_precision_loss)]
        Self::fmt_float(bytes as f64, formatter)
    }
}

impl fmt::Display for ByteSize<u64> {
    /// Human readable bytes in si units.
    ///
    /// ```
    /// use trctl::display::ByteSize;
    /// assert_eq!(format!("{}", ByteSize(0u64)), "0");
    /// assert_eq!(format!("{}", ByteSize(1023u64)), "1023");
    /// assert_eq!(format!("{}", ByteSize(1024u64)), "1.0K");
    /// assert_eq!(format!("{:5}", ByteSize(1025u64)), " 1.0K");
    /// assert_eq!(ByteSize(2047u64).to_string(), "2.0K");
    /// assert_eq!(format!("{:6.2}", ByteSize(2100u64)), " 2.05K");
    /// assert_eq!(format!("{:.0}", ByteSize(2100u64)), "2K");
    /// ```
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        let bytes = self.0;
        let width = formatter.width().unwrap_or(0);
        if bytes < 1024 {
            return write!(formatter, "{bytes:width$}");
        }
        #[allow(clippy::cast_precision_loss)]
        Self::fmt_float(bytes as f64, formatter)
    }
}

#[cfg(test)]
mod tests {
    use transmission_rpc::types::TorrentStatus;

    use super::Status;

    #[test]
    fn status() {
        let mut status = Status {
            status: TorrentStatus::QueuedToDownload,
            is_finished: Some(true),
            recheck_progress: Some(0.2),
            peers_getting_from_us: Some(3),
            peers_sending_to_us: Some(2),
            left_until_done: Some(2323),
        };
        assert_eq!(format!("{status}"), "Queued");
        assert_eq!(format!("{status:7}"), "Queued ");
        status.status = TorrentStatus::Stopped;
        assert_eq!(format!("{status}"), "Finished");
        assert_eq!(format!("{status:9}"), "Finished ");
        status.is_finished = Some(false);
        assert_eq!(status.to_string(), "Stopped");
    }
}
