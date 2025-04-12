use std::os::unix::ffi::OsStrExt;

use bendy::decoding::{FromBencode as _, ResultExt as _};
use sha1::{Digest as _, Sha1};

use crate::errors::anyhow;

#[derive(Debug)]
pub struct Torrent {
    pub info_hash: String,
    pub length: u64,
    pub name: Vec<u8>,
}

#[derive(Debug)]
enum TorrentError {
    Bendy(bendy::decoding::Error),
    Custom(&'static str),
}

impl From<bendy::decoding::Error> for TorrentError {
    fn from(error: bendy::decoding::Error) -> Self {
        TorrentError::Bendy(error)
    }
}

impl Torrent {
    pub fn from_bytes(bytes: &[u8]) -> crate::errors::Result<Self> {
        Self::from_bytes_doit(bytes).map_err(|err| match err {
            TorrentError::Bendy(e) => anyhow!("bendy: {}", e),
            TorrentError::Custom(e) => anyhow!("torrent error: {}", e),
        })
    }

    fn from_bytes_doit(bytes: &[u8]) -> Result<Self, TorrentError> {
        let mut decoder = bendy::decoding::Decoder::new(bytes);
        let mut info_hash = None;
        let mut length = None;
        let mut name = None;

        match decoder.next_object().context("next_object")? {
            None => return Err(TorrentError::Custom("eof")),
            Some(obj) => {
                let mut dict = obj.try_into_dictionary().context("torrent object")?;
                while let Some(pair) = dict.next_pair().context("dict pair")? {
                    if let (b"info", value) = pair {
                        let mut infodict = value.try_into_dictionary().context("info value")?;
                        while let Some(infopair) = infodict.next_pair().context("dict pair")? {
                            match infopair {
                                (b"name", value) => {
                                    name.replace(value.try_into_bytes().context("name")?.to_vec());
                                }
                                (b"length", value) => {
                                    length.replace(
                                        u64::decode_bencode_object(value).context("length")?,
                                    );
                                }
                                (b"files", value) => {
                                    if length.is_none() {
                                        let mut files =
                                            value.try_into_list().context("invalid list")?;
                                        let mut len: u64 = 0;
                                        while let Some(file) =
                                            files.next_object().context("next file")?
                                        {
                                            let mut file = file
                                                .try_into_dictionary()
                                                .context("invalid file")?;
                                            while let Some(pair) =
                                                file.next_pair().context("file dict pair")?
                                            {
                                                match pair {
                                                    (b"path", value) => {
                                                        if name.is_none() {
                                                            let mut path_components = value
                                                                .try_into_list()
                                                                .context("path components")?;
                                                            let mut pb = std::path::PathBuf::new();
                                                            while let Some(pc) = path_components
                                                                .next_object()
                                                                .context("path component")?
                                                            {
                                                                pb.push(std::path::Path::new(
                                                                    std::ffi::OsStr::from_bytes(
                                                                        pc.try_into_bytes()
                                                                            .context(
                                                                            "path component bytes",
                                                                        )?,
                                                                    ),
                                                                ));
                                                            }
                                                            name.replace(
                                                                pb.as_os_str().as_bytes().to_vec(),
                                                            );
                                                        }
                                                    }
                                                    (b"length", value) => {
                                                        len = len
                                                            .checked_add(
                                                                u64::decode_bencode_object(value)
                                                                    .context("invalid u64")?,
                                                            )
                                                            .expect("length overflowed");
                                                    }
                                                    (_, _) => {}
                                                }
                                            }
                                        }
                                        length.replace(len);
                                    }
                                }
                                (_, _) => {}
                            }
                        }

                        let mut hasher = Sha1::new();
                        let infobytes = infodict.into_raw().context("info dict")?;
                        hasher.update(infobytes);
                        info_hash.replace(format!("{:x}", hasher.finalize()));
                    }
                }
            }
        }

        Ok(Torrent {
            info_hash: info_hash
                .ok_or(TorrentError::Custom("info hash could not be calculated"))?,
            length: length.ok_or(TorrentError::Custom("length could not be calculated"))?,
            name: name.ok_or(TorrentError::Custom("name could not be found"))?,
        })
    }
}
