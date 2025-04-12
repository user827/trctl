use crate::client::{Client, MockRequest, QueryCmd, SyncRequest, TorrentCli, TorrentFilter};
#[cfg(test)]
use crate::console::imps::tests::{MockCon, MockReader, MockView};
use crate::console::{
    Console, Dbus, DefCon, DefLog, Notifier, ReadLine, StdLog, Unprivileged, View,
};
use crate::db::DBSqlite;
use crate::errors::*;
use crate::{Trctl, Trmv};
use byte_unit::Byte;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::path::{Path, PathBuf};
use termcolor::WriteColor;
use toml::Value;
use transmission_rpc::types::BasicAuth;
use transmission_rpc::TransClient;
use url::Url;

pub fn option_explicit_none<'de, T, D>(deserializer: D) -> std::result::Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(match Value::deserialize(deserializer)? {
        Value::String(ref value) if value.to_lowercase() == "none" => None,
        value => Some(T::deserialize(value).map_err(serde::de::Error::custom)?),
    })
}

pub fn option_explicit_serialize<T, S>(
    val: &Option<T>,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error>
where
    S: Serializer,
    T: Serialize,
{
    match val {
        None => str::serialize("none", serializer),
        Some(ref val) => T::serialize(val, serializer),
    }
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
#[allow(clippy::struct_excessive_bools)]
pub struct Config {
    #[serde(serialize_with = "option_explicit_serialize")]
    #[serde(deserialize_with = "option_explicit_none")]
    pub mailuser: Option<String>,
    pub rpc_url: Url,
    #[serde(serialize_with = "option_explicit_serialize")]
    #[serde(deserialize_with = "option_explicit_none")]
    pub rpc_user: Option<String>,
    #[serde(serialize_with = "option_explicit_serialize")]
    #[serde(deserialize_with = "option_explicit_none")]
    pub rpc_pass: Option<String>,
    pub force_not_remote: bool,
    pub base_dir: PathBuf,
    pub sqlitedb: bool,
    pub destination_dirs: Vec<PathBuf>,
    pub default_destination: PathBuf,
    pub verify: bool,
    pub dldirs: Vec<PathBuf>,
    pub ask_existing: bool,
    #[serde(serialize_with = "option_explicit_serialize")]
    #[serde(deserialize_with = "option_explicit_none")]
    pub copydir: Option<PathBuf>,
    pub quota_per_dldir: Byte,
    pub free_space_per_dldir: Byte,
    pub dst_free_space_to_leave: Byte,
    #[serde(serialize_with = "option_explicit_serialize")]
    #[serde(deserialize_with = "option_explicit_none")]
    pub color: Option<bool>,
}

pub type Def = Builder<SyncRequest>;

impl Default for Config {
    fn default() -> Self {
        Self {
            mailuser: None,
            color: None,
            rpc_url: Url::parse("http://127.0.0.1:9091/transmission/rpc").unwrap(),
            rpc_user: None,
            rpc_pass: None,
            force_not_remote: false,
            base_dir: PathBuf::from("/var/cache/torrents/"),
            verify: false,
            default_destination: PathBuf::from("/var/cache/torrents/completed"),
            // TODO
            sqlitedb: true,
            destination_dirs: ["/var/cache/torrents/completed"]
                .into_iter()
                .map(PathBuf::from)
                .collect(),
            dldirs: ["/var/cache/torrents/dl"]
                .iter()
                .map(PathBuf::from)
                .collect(),
            ask_existing: true,
            copydir: None,
            quota_per_dldir: Byte::parse_str("100GiB", true).expect("Invalid byte"),
            free_space_per_dldir: Byte::parse_str("100GiB", true).expect("Invalid byte"),
            dst_free_space_to_leave: Byte::parse_str("40GiB", true).expect("Invalid byte"),
        }
    }
}

impl Config {
    pub fn load(name: &str) -> Result<Self> {
        confy::load(name, Some("config")).context("Config")
    }

    pub fn load_path(path: impl AsRef<Path>) -> Result<Self> {
        confy::load_path(path).context("Config")
    }

    pub fn config_path(name: &str) -> Result<PathBuf> {
        confy::get_configuration_file_path(name, Some("config")).context("config path")
    }

    pub fn builder(self, name: &str) -> Def {
        self.builder_with(Builder::default_client, name.to_string())
    }

    pub fn builder_with<C: TorrentCli>(
        self,
        fclient: fn(&Builder<C>) -> Result<C>,
        name: String,
    ) -> Builder<C> {
        Builder {
            cfg: self,
            fclient,
            interactive: true,
            name,
        }
    }

    #[must_use]
    pub fn get(name: &str) -> Builder<SyncRequest> {
        Builder {
            cfg: Config::default(),
            fclient: Builder::default_client,
            interactive: true,
            name: name.to_string(),
        }
    }

    #[must_use]
    pub fn get_mock() -> Builder<MockRequest> {
        Builder {
            cfg: Config::default(),
            fclient: Builder::mock_client,
            interactive: true,
            name: "mockman".to_string(),
        }
    }
}

pub struct Builder<C> {
    pub cfg: Config,
    fclient: fn(&Self) -> Result<C>,
    pub interactive: bool,
    name: String,
}

#[cfg(test)]
use termcolor::Buffer;
#[cfg(test)]
impl Builder<MockRequest> {
    pub fn mock_log(&self) -> Result<StdLog<Buffer>> {
        Ok(MockView::default())
    }

    pub fn mock_trctl(self, log: StdLog<Buffer>) -> Result<Trctl<MockRequest, MockCon>> {
        let client = self.new_client()?;
        Ok(Trctl {
            dldirs: self.cfg.dldirs,
            dst_free_space_to_leave: 10,
            interactive: true,
            is_remote: false,
            client,
            verify: self.cfg.verify,
            destination_dirs: self.cfg.destination_dirs,
            default_destination: self.cfg.default_destination,
            console: Console {
                v_ask_existing: true,
                base_dir: self.cfg.base_dir,
                log,
                input: MockReader {
                    input: String::new(),
                    input_pos: 0,
                },
            },
        })
    }
}

#[derive(Clone, Copy)]
pub struct BuilderOpts {
    pub interactive: bool,
}

impl<C: TorrentCli> Builder<C> {
    #[allow(clippy::unused_self)]
    #[must_use]
    pub fn new_notifier_dbus(&self, name: String) -> Notifier<Dbus> {
        Notifier::new(Dbus::new(name.clone(), self.cfg.ask_existing), name)
    }

    #[cfg(feature = "sqlite")]
    pub fn sqlitedbpath(&self) -> Result<PathBuf> {
        let xdg_dirs = xdg::BaseDirectories::with_prefix(&self.name)?;
        Ok(xdg_dirs.place_data_file("fetched.sqlite3")?)
    }

    pub fn set_cli_opts(&mut self, opts: BuilderOpts) {
        self.interactive = opts.interactive;
    }

    pub fn new_notifier_email(&self, name: String) -> Result<Notifier<Unprivileged>> {
        let touser = std::env::var("TRMV_NOTIFYADDR").context("no TRMV_NOTIFYADDR set")?;
        Ok(Notifier::new(
            Unprivileged::new(&touser, name.clone())?,
            name,
        ))
    }

    pub fn default_client(&self) -> Result<SyncRequest> {
        let client = self.new_transmission()?;
        let tokio = tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .build()?;
        Ok(SyncRequest { client, tokio })
    }

    pub fn new_filter<'h>(&'h self, qcmd: &'h QueryCmd) -> Result<TorrentFilter<'h>> {
        TorrentFilter::new(self.cfg.dldirs.as_slice(), qcmd)
    }

    #[allow(clippy::unused_self)]
    pub fn mock_client(&self) -> Result<MockRequest> {
        Ok(MockRequest::default())
    }

    pub fn new_client(&self) -> Result<Client<C>> {
        Ok(Client {
            imp: (self.fclient)(self)?,
            dldirs: self.cfg.dldirs.clone(),
        })
    }

    fn is_remote(url: &Url, force_not_remote: bool) -> bool {
        if force_not_remote {
            return false;
        }
        let host = url.host_str();
        if let Some(h) = host {
            if h.starts_with("127.") || h == "localhost" || h == "::1" {
                return false;
            }
        }
        true
    }

    pub fn new_trctl_input<IO: WriteColor, I: ReadLine>(
        self,
        log: StdLog<IO>,
        input: I,
    ) -> std::result::Result<Trctl<C, Console<IO, I>>, Error> {
        let client = self.new_client()?;
        Ok(Trctl {
            interactive: self.interactive,
            verify: self.cfg.verify,
            dldirs: self.cfg.dldirs,
            is_remote: Self::is_remote(&self.cfg.rpc_url, self.cfg.force_not_remote),
            destination_dirs: self.cfg.destination_dirs,
            default_destination: self.cfg.default_destination,
            dst_free_space_to_leave: self.cfg.dst_free_space_to_leave.as_u64(),
            console: Console {
                v_ask_existing: self.cfg.ask_existing,
                base_dir: self.cfg.base_dir,
                log,
                input,
            },
            client,
        })
    }

    //pub fn new_trmv_from_log<IO: IOImp, I: ReadLine>(
    //    self,
    //    log: &mut StdLog<IO>,
    //    input: I,
    //) -> std::result::Result<Trmv<C, Console<IO, I>>, Error> {
    //    let base_dir = self.cfg.base_dir.clone();
    //    self.new_trmv(Console {
    //        log,
    //        base_dir,
    //        input,
    //    })
    //}

    pub fn new_trctl(self, log: DefLog) -> Result<Trctl<C, DefCon>> {
        self.new_trctl_input(log, std::io::stdin())
    }

    /// # Panics
    /// does not
    pub fn new_trmv(self, log: DefLog) -> Result<Trmv<C, DefCon>> {
        let v = Console {
            log,
            base_dir: self.cfg.base_dir.clone(),
            input: std::io::stdin(),
            v_ask_existing: self.cfg.ask_existing,
        };
        self.new_trmv_view(v)
    }

    pub fn new_trmv_view<V: View>(self, view: V) -> std::result::Result<Trmv<C, V>, Error> {
        #[cfg(feature = "sqlite")]
        let db = DBSqlite::new(if self.cfg.sqlitedb {
            Some(self.sqlitedbpath()?)
        } else {
            None
        });
        Ok(Trmv {
            client: self.new_client()?,
            view,
            copydir: self.cfg.copydir,
            base_dir: self.cfg.base_dir,
            dldirs: self.cfg.dldirs,
            quota: self.cfg.quota_per_dldir.as_u64(),
            safe_space: self.cfg.free_space_per_dldir.as_u64(),
            #[cfg(feature = "sqlite")]
            db,
        })
    }

    pub fn new_transmission(&self) -> Result<TransClient> {
        if let (Some(user), Some(password)) = (&self.cfg.rpc_user, &self.cfg.rpc_pass) {
            let basic_auth = BasicAuth {
                // Ku ei me jakseta muistaa miten te kaikki funkkarit ootte konfiggia kopeloineet niin
                // sovitaan että pidetään se koskemattomana
                user: user.clone(),
                password: password.clone(),
            };
            Ok(TransClient::with_auth(self.cfg.rpc_url.clone(), basic_auth))
        } else {
            Ok(TransClient::new(self.cfg.rpc_url.clone()))
        }
    }
}

//    // Trying singleton
//    //pub fn get_console(&mut self) -> &mut Console {
//    //    if self.console.is_none() {
//    //        self.console.replace(self.new_console());
//    //    }
//    //    self.console.as_mut().unwrap()
//    //}
