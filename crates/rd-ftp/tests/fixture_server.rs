//! A minimal in-process FTP server, speaking only the command subset `rd-ftp` uses.
//!
//! Real FTP servers cannot be assumed present in CI, and a fixture also makes the failure
//! cases (a refused `REST`, a file that changed between attempts) reproducible, which is
//! exactly what the resume rules need to be tested against.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

/// One file the fixture serves.
#[derive(Clone)]
pub struct RemoteFile {
    pub content: Vec<u8>,
    /// `MDTM` reply in `YYYYMMDDHHMMSS` form.
    pub modified: String,
}

impl RemoteFile {
    pub fn new(content: impl Into<Vec<u8>>) -> Self {
        Self {
            content: content.into(),
            modified: "20260101120000".to_owned(),
        }
    }

    pub fn modified_at(mut self, stamp: &str) -> Self {
        self.modified = stamp.to_owned();
        self
    }
}

/// Behaviour switches for the negative cases.
#[derive(Clone, Default)]
pub struct Behaviour {
    /// Refuse `REST`, as servers without resume support do.
    pub refuse_rest: bool,
    /// Leave `REST` out of the `FEAT` reply.
    pub hide_rest_feature: bool,
    /// Answer `MLSD` with an error so the client falls back to `LIST`.
    pub refuse_mlsd: bool,
    /// Close the data connection after this many bytes, simulating a dropped transfer.
    pub truncate_after: Option<usize>,
}

#[derive(Clone)]
pub struct Fixture {
    pub files: Arc<Mutex<HashMap<String, RemoteFile>>>,
    pub directories: Arc<Mutex<Vec<String>>>,
    pub behaviour: Arc<Mutex<Behaviour>>,
    pub port: u16,
}

impl Fixture {
    /// Starts the server on an ephemeral port and returns once it is accepting.
    pub async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let fixture = Self {
            files: Arc::new(Mutex::new(HashMap::new())),
            directories: Arc::new(Mutex::new(Vec::new())),
            behaviour: Arc::new(Mutex::new(Behaviour::default())),
            port,
        };
        let served = fixture.clone();
        tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                let session = served.clone();
                tokio::spawn(async move {
                    let _ = session.serve(socket).await;
                });
            }
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

    fn behaviour(&self) -> Behaviour {
        self.behaviour.lock().expect("behaviour").clone()
    }

    fn file(&self, path: &str) -> Option<RemoteFile> {
        self.files.lock().expect("files").get(path).cloned()
    }

    fn is_directory(&self, path: &str) -> bool {
        let path = path.trim_end_matches('/');
        let path = if path.is_empty() { "/" } else { path };
        path == "/"
            || self
                .directories
                .lock()
                .expect("dirs")
                .iter()
                .any(|d| d == path)
    }

    /// Entries directly below `path`.
    fn children(&self, path: &str) -> Vec<(String, Option<RemoteFile>)> {
        let prefix = format!("{}/", path.trim_end_matches('/'));
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

    async fn serve(&self, socket: TcpStream) -> std::io::Result<()> {
        let (read_half, mut write) = socket.into_split();
        let mut reader = BufReader::new(read_half);
        write.write_all(b"220 rdownloader test server\r\n").await?;

        let mut working_directory = "/".to_owned();
        let mut rest_offset = 0usize;
        let mut pending_data: Option<TcpListener> = None;
        let mut line = String::new();

        loop {
            line.clear();
            if reader.read_line(&mut line).await? == 0 {
                return Ok(());
            }
            let trimmed = line.trim_end();
            let (command, argument) = trimmed.split_once(' ').unwrap_or((trimmed, ""));
            let command = command.to_ascii_uppercase();

            match command.as_str() {
                "USER" => write.write_all(b"331 need password\r\n").await?,
                "PASS" => write.write_all(b"230 logged in\r\n").await?,
                "TYPE" => write.write_all(b"200 type set\r\n").await?,
                "SYST" => write.write_all(b"215 UNIX Type: L8\r\n").await?,
                "FEAT" => {
                    let mut reply = String::from("211-Features:\r\n");
                    if !self.behaviour().hide_rest_feature {
                        reply.push_str(" REST STREAM\r\n");
                    }
                    reply.push_str(" MLSD\r\n211 End\r\n");
                    write.write_all(reply.as_bytes()).await?;
                }
                "PWD" => {
                    write
                        .write_all(format!("257 \"{working_directory}\"\r\n").as_bytes())
                        .await?;
                }
                "CWD" => {
                    let target = self.absolute(&working_directory, argument);
                    if self.is_directory(&target) {
                        working_directory = target;
                        write.write_all(b"250 directory changed\r\n").await?;
                    } else {
                        write.write_all(b"550 no such directory\r\n").await?;
                    }
                }
                "SIZE" => {
                    let target = self.absolute(&working_directory, argument);
                    match self.file(&target) {
                        Some(file) => {
                            let reply = format!("213 {}\r\n", file.content.len());
                            write.write_all(reply.as_bytes()).await?;
                        }
                        None => write.write_all(b"550 not a plain file\r\n").await?,
                    }
                }
                "MDTM" => {
                    let target = self.absolute(&working_directory, argument);
                    match self.file(&target) {
                        Some(file) => {
                            let reply = format!("213 {}\r\n", file.modified);
                            write.write_all(reply.as_bytes()).await?;
                        }
                        None => write.write_all(b"550 not found\r\n").await?,
                    }
                }
                "PASV" => {
                    let data = TcpListener::bind("127.0.0.1:0").await?;
                    let port = data.local_addr()?.port();
                    pending_data = Some(data);
                    let reply = format!(
                        "227 Entering Passive Mode (127,0,0,1,{},{})\r\n",
                        port / 256,
                        port % 256
                    );
                    write.write_all(reply.as_bytes()).await?;
                }
                "EPSV" => {
                    let data = TcpListener::bind("127.0.0.1:0").await?;
                    let port = data.local_addr()?.port();
                    pending_data = Some(data);
                    let reply = format!("229 Entering Extended Passive Mode (|||{port}|)\r\n");
                    write.write_all(reply.as_bytes()).await?;
                }
                "REST" => {
                    if self.behaviour().refuse_rest {
                        write.write_all(b"502 REST not implemented\r\n").await?;
                    } else {
                        rest_offset = argument.trim().parse().unwrap_or(0);
                        write.write_all(b"350 restarting\r\n").await?;
                    }
                }
                "MLSD" | "LIST" => {
                    if command == "MLSD" && self.behaviour().refuse_mlsd {
                        write.write_all(b"500 MLSD not understood\r\n").await?;
                        continue;
                    }
                    let target = if argument.is_empty() {
                        working_directory.clone()
                    } else {
                        self.absolute(&working_directory, argument)
                    };
                    let body = self.render_listing(&target, &command);
                    let Some(data) = pending_data.take() else {
                        write.write_all(b"425 no data connection\r\n").await?;
                        continue;
                    };
                    write.write_all(b"150 opening data connection\r\n").await?;
                    if let Ok((mut stream, _)) = data.accept().await {
                        stream.write_all(body.as_bytes()).await?;
                        stream.shutdown().await?;
                    }
                    write.write_all(b"226 transfer complete\r\n").await?;
                }
                "RETR" => {
                    let target = self.absolute(&working_directory, argument);
                    let Some(file) = self.file(&target) else {
                        write.write_all(b"550 not found\r\n").await?;
                        continue;
                    };
                    let Some(data) = pending_data.take() else {
                        write.write_all(b"425 no data connection\r\n").await?;
                        continue;
                    };
                    let offset = rest_offset.min(file.content.len());
                    rest_offset = 0;
                    let mut payload = file.content[offset..].to_vec();
                    let truncated = self.behaviour().truncate_after;
                    if let Some(limit) = truncated {
                        payload.truncate(limit);
                    }
                    write.write_all(b"150 opening data connection\r\n").await?;
                    if let Ok((mut stream, _)) = data.accept().await {
                        stream.write_all(&payload).await?;
                        stream.shutdown().await?;
                    }
                    write.write_all(b"226 transfer complete\r\n").await?;
                }
                "QUIT" => {
                    write.write_all(b"221 bye\r\n").await?;
                    return Ok(());
                }
                _ => write.write_all(b"502 not implemented\r\n").await?,
            }
        }
    }

    fn render_listing(&self, path: &str, command: &str) -> String {
        let mut body = String::new();
        if command == "LIST" {
            body.push_str("total 8\r\n");
        }
        for (name, file) in self.children(path) {
            match (command, &file) {
                ("MLSD", Some(file)) => body.push_str(&format!(
                    "type=file;size={};modify={}; {name}\r\n",
                    file.content.len(),
                    file.modified
                )),
                ("MLSD", None) => body.push_str(&format!("type=dir; {name}\r\n")),
                (_, Some(file)) => body.push_str(&format!(
                    "-rw-r--r-- 1 owner group {} Jan 10 12:00 {name}\r\n",
                    file.content.len()
                )),
                (_, None) => {
                    body.push_str(&format!(
                        "drwxr-xr-x 2 owner group 4096 Jan 10 12:00 {name}\r\n"
                    ));
                }
            }
        }
        body
    }

    /// Resolves an argument against the session's working directory.
    fn absolute(&self, working_directory: &str, argument: &str) -> String {
        let argument = argument.trim();
        if argument.starts_with('/') {
            return argument.trim_end_matches('/').to_owned().max_root();
        }
        format!("{}/{argument}", working_directory.trim_end_matches('/'))
    }
}

/// Keeps the server root addressable as `/` rather than the empty string.
trait MaxRoot {
    fn max_root(self) -> String;
}

impl MaxRoot for String {
    fn max_root(self) -> String {
        if self.is_empty() {
            "/".to_owned()
        } else {
            self
        }
    }
}
