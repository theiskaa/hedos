//! What the engine installers' tests share: a scratch directory, a tar
//! written entry by entry (hostile paths included), and a release host.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use flate2::Compression;
use flate2::write::GzEncoder;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// A directory of its own under the temp dir, removed when dropped.
pub(crate) struct Scratch(pub(crate) PathBuf);

impl Scratch {
    pub(crate) fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "hedos-engine-{name}-{}-{}",
            std::process::id(),
            kernel::time::now_millis()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// One tar entry: a path written as is (so `..` and absolute paths can be
/// tested), and a file's bytes, a directory, or a link's target.
pub(crate) enum Item<'a> {
    File(&'a str, &'a [u8], u32),
    Dir(&'a str),
    Link(&'a str, &'a str),
}

/// A gzipped tar of `items`.
pub(crate) fn archive(items: &[Item]) -> Vec<u8> {
    let mut builder = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::fast()));
    for item in items {
        let mut header = tar::Header::new_old();
        let (path, data, kind, mode): (&str, &[u8], tar::EntryType, u32) = match item {
            Item::File(path, data, mode) => (path, data, tar::EntryType::Regular, *mode),
            Item::Dir(path) => (path, &[], tar::EntryType::Directory, 0o755),
            Item::Link(path, target) => {
                header.set_link_name(target).unwrap();
                (path, &[], tar::EntryType::Symlink, 0o777)
            }
        };
        let name = &mut header.as_old_mut().name;
        name[..path.len()].copy_from_slice(path.as_bytes());
        header.set_entry_type(kind);
        header.set_mode(mode);
        header.set_size(data.len() as u64);
        header.set_cksum();
        builder.append(&header, data).unwrap();
    }
    builder.into_inner().unwrap().finish().unwrap()
}

/// Serve `files` by name over HTTP until dropped; returns the base URL.
pub(crate) async fn serve(
    files: HashMap<&'static str, Vec<u8>>,
) -> (String, tokio::task::AbortHandle) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    let files = Arc::new(files);
    let accept = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let files = Arc::clone(&files);
            tokio::spawn(async move {
                let mut request = Vec::new();
                let mut buffer = [0u8; 1024];
                while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                    let Ok(read) = stream.read(&mut buffer).await else {
                        return;
                    };
                    if read == 0 {
                        return;
                    }
                    request.extend_from_slice(&buffer[..read]);
                }
                let head = String::from_utf8_lossy(&request);
                let path = head.split_whitespace().nth(1).unwrap_or("/");
                let response = match files.get(path.trim_start_matches('/')) {
                    Some(body) => {
                        let mut response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        )
                        .into_bytes();
                        response.extend_from_slice(body);
                        response
                    }
                    None => b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n".to_vec(),
                };
                let _ = stream.write_all(&response).await;
            });
        }
    })
    .abort_handle();
    (base, accept)
}
