//! An in-process SSH server with an SFTP subsystem, for testing the runner end to end.
//!
//! A real SSH daemon cannot be assumed present in CI, and only a fixture lets the host-key
//! cases be exercised at all: rotating a server's identity between two connection attempts
//! is exactly what the trust store has to notice.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use russh::keys::PrivateKey;
use russh::server::{Auth, Msg, Server as _, Session};
use russh::{Channel, ChannelId};
use russh_sftp::protocol::{
    Attrs, File as SftpFile, FileAttributes, Handle, Name, Status, StatusCode, Version,
};
use tokio::net::TcpListener;

/// One file the fixture serves.
#[derive(Clone)]
pub struct RemoteFile {
    pub content: Vec<u8>,
    /// Modification time in seconds since the epoch.
    pub modified: u32,
}

impl RemoteFile {
    pub fn new(content: impl Into<Vec<u8>>) -> Self {
        Self {
            content: content.into(),
            modified: 1_767_268_800,
        }
    }

    pub fn modified_at(mut self, modified: u32) -> Self {
        self.modified = modified;
        self
    }
}

#[derive(Clone, Default)]
pub struct Behaviour {
    /// Stop serving a file after this many bytes, simulating a dropped transfer.
    pub truncate_after: Option<usize>,
    /// Reject every password, to exercise the auth failure path.
    pub reject_password: bool,
}

#[derive(Clone)]
pub struct Fixture {
    pub files: Arc<Mutex<HashMap<String, RemoteFile>>>,
    pub directories: Arc<Mutex<Vec<String>>>,
    pub behaviour: Arc<Mutex<Behaviour>>,
    pub port: u16,
}

impl Fixture {
    /// Starts a server whose host identity is `host_key`.
    ///
    /// The key is passed in so a test can restart the fixture on the same port with a
    /// different identity, which is what a changed host key looks like to the client.
    pub async fn start(host_key: PrivateKey) -> Self {
        Self::start_on(host_key, None).await
    }

    pub async fn start_on(host_key: PrivateKey, port: Option<u16>) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", port.unwrap_or(0)))
            .await
            .expect("bind");
        let bound = listener.local_addr().expect("addr").port();
        let fixture = Self {
            files: Arc::new(Mutex::new(HashMap::new())),
            directories: Arc::new(Mutex::new(Vec::new())),
            behaviour: Arc::new(Mutex::new(Behaviour::default())),
            port: bound,
        };
        let config = Arc::new(russh::server::Config {
            keys: vec![host_key],
            ..russh::server::Config::default()
        });
        let mut server = SshServer {
            fixture: fixture.clone(),
        };
        tokio::spawn(async move {
            let _ = server.run_on_socket(config, &listener).await;
            // Keep the listener alive for the task's lifetime.
            drop(listener);
        });
        fixture
    }

    pub fn put(&self, path: &str, file: RemoteFile) {
        self.files
            .lock()
            .expect("files")
            .insert(path.to_owned(), file);
    }

    pub fn put_directory(&self, path: &str) {
        self.directories.lock().expect("dirs").push(path.to_owned());
    }

    pub fn set_behaviour(&self, behaviour: Behaviour) {
        *self.behaviour.lock().expect("behaviour") = behaviour;
    }

    fn file(&self, path: &str) -> Option<RemoteFile> {
        self.files.lock().expect("files").get(path).cloned()
    }

    fn is_directory(&self, path: &str) -> bool {
        let path = normalize(path);
        path == "/"
            || self
                .directories
                .lock()
                .expect("dirs")
                .iter()
                .any(|entry| entry == &path)
    }

    fn children(&self, path: &str) -> Vec<(String, Option<RemoteFile>)> {
        let prefix = format!("{}/", normalize(path).trim_end_matches('/'));
        let mut out = Vec::new();
        for (candidate, file) in self.files.lock().expect("files").iter() {
            if let Some(rest) = candidate.strip_prefix(&prefix)
                && !rest.contains('/')
            {
                out.push((rest.to_owned(), Some(file.clone())));
            }
        }
        for candidate in self.directories.lock().expect("dirs").iter() {
            if let Some(rest) = candidate.strip_prefix(&prefix)
                && !rest.is_empty()
                && !rest.contains('/')
            {
                out.push((rest.to_owned(), None));
            }
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }
}

fn normalize(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        "/".to_owned()
    } else {
        trimmed.to_owned()
    }
}

struct SshServer {
    fixture: Fixture,
}

impl russh::server::Server for SshServer {
    type Handler = SshSession;

    fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> SshSession {
        SshSession {
            fixture: self.fixture.clone(),
            channels: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
        }
    }
}

struct SshSession {
    fixture: Fixture,
    /// Open channels, so `subsystem_request` can take the one it was asked about; the
    /// callback only receives an id.
    channels: Arc<tokio::sync::Mutex<HashMap<ChannelId, Channel<Msg>>>>,
}

impl russh::server::Handler for SshSession {
    type Error = russh::Error;

    async fn auth_password(&mut self, _user: &str, _password: &str) -> Result<Auth, Self::Error> {
        if self
            .fixture
            .behaviour
            .lock()
            .expect("behaviour")
            .reject_password
        {
            return Ok(Auth::reject());
        }
        Ok(Auth::Accept)
    }

    async fn auth_publickey(
        &mut self,
        _user: &str,
        _key: &russh::keys::PublicKey,
    ) -> Result<Auth, Self::Error> {
        Ok(Auth::Accept)
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: russh::server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.channels.lock().await.insert(channel.id(), channel);
        reply.accept().await;
        Ok(())
    }

    async fn subsystem_request(
        &mut self,
        channel_id: ChannelId,
        name: &str,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        if name != "sftp" {
            session.channel_failure(channel_id)?;
            return Ok(());
        }
        let Some(channel) = self.channels.lock().await.remove(&channel_id) else {
            session.channel_failure(channel_id)?;
            return Ok(());
        };
        session.channel_success(channel_id)?;
        let handler = SftpHandler {
            fixture: self.fixture.clone(),
            handles: HashMap::new(),
        };
        tokio::spawn(async move {
            russh_sftp::server::run(channel.into_stream(), handler).await;
        });
        Ok(())
    }
}

/// What an open handle points at.
enum Opened {
    File(String),
    /// A directory listing, with `drained` marking that READDIR already returned it once.
    Dir {
        path: String,
        drained: bool,
    },
}

struct SftpHandler {
    fixture: Fixture,
    handles: HashMap<String, Opened>,
}

impl SftpHandler {
    fn attributes(file: &RemoteFile) -> FileAttributes {
        let mut attributes = FileAttributes {
            size: Some(file.content.len() as u64),
            mtime: Some(file.modified),
            atime: Some(file.modified),
            ..FileAttributes::default()
        };
        attributes.set_dir(false);
        attributes.set_regular(true);
        attributes
    }

    fn directory_attributes() -> FileAttributes {
        let mut attributes = FileAttributes::default();
        attributes.set_dir(true);
        attributes
    }
}

impl russh_sftp::server::Handler for SftpHandler {
    type Error = StatusCode;

    fn unimplemented(&self) -> Self::Error {
        StatusCode::OpUnsupported
    }

    async fn init(
        &mut self,
        _version: u32,
        _extensions: HashMap<String, String>,
    ) -> Result<Version, Self::Error> {
        Ok(Version::new())
    }

    async fn realpath(&mut self, id: u32, path: String) -> Result<Name, Self::Error> {
        Ok(Name {
            id,
            files: vec![SftpFile::dummy(normalize(&path))],
        })
    }

    async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        let path = normalize(&path);
        if let Some(file) = self.fixture.file(&path) {
            return Ok(Attrs {
                id,
                attrs: Self::attributes(&file),
            });
        }
        if self.fixture.is_directory(&path) {
            return Ok(Attrs {
                id,
                attrs: Self::directory_attributes(),
            });
        }
        Err(StatusCode::NoSuchFile)
    }

    async fn lstat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        self.stat(id, path).await
    }

    async fn open(
        &mut self,
        id: u32,
        filename: String,
        _pflags: russh_sftp::protocol::OpenFlags,
        _attrs: FileAttributes,
    ) -> Result<Handle, Self::Error> {
        let path = normalize(&filename);
        if self.fixture.file(&path).is_none() {
            return Err(StatusCode::NoSuchFile);
        }
        let handle = format!("f{id}");
        self.handles.insert(handle.clone(), Opened::File(path));
        Ok(Handle { id, handle })
    }

    async fn read(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        len: u32,
    ) -> Result<russh_sftp::protocol::Data, Self::Error> {
        let Some(Opened::File(path)) = self.handles.get(&handle) else {
            return Err(StatusCode::Failure);
        };
        let file = self.fixture.file(path).ok_or(StatusCode::NoSuchFile)?;
        // A truncating fixture stops handing out bytes past the limit, which the client
        // sees as a short file rather than as an error.
        let available = self
            .fixture
            .behaviour
            .lock()
            .expect("behaviour")
            .truncate_after
            .map_or(file.content.len(), |limit| limit.min(file.content.len()));
        let start = offset as usize;
        if start >= available {
            return Err(StatusCode::Eof);
        }
        let end = available.min(start + len as usize);
        Ok(russh_sftp::protocol::Data {
            id,
            data: file.content[start..end].to_vec(),
        })
    }

    async fn close(&mut self, id: u32, handle: String) -> Result<Status, Self::Error> {
        self.handles.remove(&handle);
        Ok(Status {
            id,
            status_code: StatusCode::Ok,
            error_message: String::new(),
            language_tag: "en-US".to_owned(),
        })
    }

    async fn opendir(&mut self, id: u32, path: String) -> Result<Handle, Self::Error> {
        let path = normalize(&path);
        if !self.fixture.is_directory(&path) {
            return Err(StatusCode::NoSuchFile);
        }
        let handle = format!("d{id}");
        self.handles.insert(
            handle.clone(),
            Opened::Dir {
                path,
                drained: false,
            },
        );
        Ok(Handle { id, handle })
    }

    async fn readdir(&mut self, id: u32, handle: String) -> Result<Name, Self::Error> {
        let Some(Opened::Dir { path, drained }) = self.handles.get_mut(&handle) else {
            return Err(StatusCode::Failure);
        };
        // The protocol expects READDIR to be called until it reports EOF.
        if *drained {
            return Err(StatusCode::Eof);
        }
        *drained = true;
        let path = path.clone();
        let files = self
            .fixture
            .children(&path)
            .into_iter()
            .map(|(name, file)| {
                let attrs = file
                    .as_ref()
                    .map_or_else(Self::directory_attributes, Self::attributes);
                let mut entry = SftpFile::dummy(name);
                entry.attrs = attrs;
                entry
            })
            .collect();
        Ok(Name { id, files })
    }
}
