use crate::{
    cef_layer::{close_all_browsers, post_ui, SharedState},
    ops::handle_request,
};
use browser_protocol::{ClientFrame, ReplyResult, ServerFrame, MAX_FRAME_BYTES, PROTOCOL_VERSION};
use std::{
    fs,
    io::Read,
    os::unix::{
        fs::{FileTypeExt, PermissionsExt},
        net::UnixListener as StdUnixListener,
    },
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixListener,
    sync::mpsc,
    task::JoinHandle,
};

#[derive(Clone)]
pub struct Shutdown {
    triggered: Arc<AtomicBool>,
    shared: SharedState,
}

impl Shutdown {
    pub fn new(shared: SharedState) -> Self {
        Self {
            triggered: Arc::new(AtomicBool::new(false)),
            shared,
        }
    }

    pub fn trigger(&self) {
        if self.triggered.swap(true, Ordering::SeqCst) {
            return;
        }
        let shared = self.shared.clone();
        let _ = post_ui(move || {
            close_all_browsers(&shared);
            cef::quit_message_loop();
        });
    }
}

pub fn install_stdin_watch(shutdown: Shutdown) {
    thread::spawn(move || {
        let mut stdin = std::io::stdin();
        let mut byte = [0_u8; 1];
        loop {
            match stdin.read(&mut byte) {
                Ok(0) | Err(_) => {
                    shutdown.trigger();
                    return;
                }
                Ok(_) => {}
            }
        }
    });
}

fn prepare_socket(path: &Path) -> Result<StdUnixListener, String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create socket directory '{}': {error}", parent.display()))?;
    }
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_socket() => {
            fs::remove_file(path)
                .map_err(|error| format!("cannot replace socket '{}': {error}", path.display()))?;
        }
        Ok(_) => {
            return Err(format!(
                "socket path '{}' exists and is not a Unix socket",
                path.display()
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!("cannot inspect socket '{}': {error}", path.display()));
        }
    }

    let listener = StdUnixListener::bind(path)
        .map_err(|error| format!("cannot bind socket '{}': {error}", path.display()))?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("cannot set socket permissions on '{}': {error}", path.display()))?;
    listener
        .set_nonblocking(true)
        .map_err(|error| format!("cannot make socket nonblocking: {error}"))?;
    Ok(listener)
}

pub fn spawn_socket_server(
    path: PathBuf,
    shared: SharedState,
    shutdown: Shutdown,
) -> Result<(), String> {
    let std_listener = prepare_socket(&path)?;
    thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("blob-browser-io")
            .build();
        let Ok(runtime) = runtime else {
            eprintln!("failed to create socket runtime");
            shutdown.trigger();
            return;
        };
        runtime.block_on(async move {
            let listener = match UnixListener::from_std(std_listener) {
                Ok(listener) => listener,
                Err(error) => {
                    eprintln!("failed to adopt Unix socket: {error}");
                    shutdown.trigger();
                    return;
                }
            };
            tokio::spawn(watch_signals(shutdown.clone()));

            let mut connection: Option<JoinHandle<()>> = None;
            loop {
                match listener.accept().await {
                    Ok((stream, _)) => {
                        if let Some(previous) = connection.take() {
                            previous.abort();
                        }
                        connection = Some(tokio::spawn(handle_connection(
                            stream,
                            shared.clone(),
                        )));
                    }
                    Err(error) => {
                        eprintln!("Unix socket accept failed: {error}");
                        shutdown.trigger();
                        return;
                    }
                }
            }
        });
    });
    Ok(())
}

async fn watch_signals(shutdown: Shutdown) {
    use tokio::signal::unix::{signal, SignalKind};
    let Ok(mut interrupt) = signal(SignalKind::interrupt()) else {
        return;
    };
    let Ok(mut terminate) = signal(SignalKind::terminate()) else {
        return;
    };
    tokio::select! {
        _ = interrupt.recv() => {}
        _ = terminate.recv() => {}
    }
    shutdown.trigger();
}

struct AbortOnDrop<T>(JoinHandle<T>);

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

fn frame_bytes(frame: &ServerFrame) -> Result<Vec<u8>, String> {
    let mut bytes = serde_json::to_vec(frame).map_err(|error| error.to_string())?;
    bytes.push(b'\n');
    if bytes.len() > MAX_FRAME_BYTES {
        if let ServerFrame::Reply { id, .. } = frame {
            let replacement = ServerFrame::Reply {
                id: *id,
                result: ReplyResult::Error(format!(
                    "reply exceeds the {} byte frame limit",
                    MAX_FRAME_BYTES
                )),
            };
            bytes = serde_json::to_vec(&replacement).map_err(|error| error.to_string())?;
            bytes.push(b'\n');
        } else {
            return Err(format!("frame exceeds the {} byte limit", MAX_FRAME_BYTES));
        }
    }
    Ok(bytes)
}

async fn write_loop(
    mut writer: tokio::net::unix::OwnedWriteHalf,
    mut receiver: mpsc::Receiver<ServerFrame>,
) {
    while let Some(frame) = receiver.recv().await {
        let Ok(bytes) = frame_bytes(&frame) else {
            return;
        };
        if writer.write_all(&bytes).await.is_err() {
            return;
        }
    }
}

async fn handle_connection(stream: tokio::net::UnixStream, shared: SharedState) {
    let (reader, writer) = stream.into_split();
    let (outgoing, receiver) = mpsc::channel::<ServerFrame>(64);
    let writer = tokio::spawn(write_loop(writer, receiver));
    let _writer = AbortOnDrop(writer);
    let mut reader = tokio::io::BufReader::new(reader);
    let mut pending = Vec::new();
    let mut chunk = [0_u8; 8192];
    let mut hello_complete = false;

    loop {
        let read = match reader.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(read) => read,
        };
        pending.extend_from_slice(&chunk[..read]);
        if pending.len() > MAX_FRAME_BYTES {
            return;
        }

        while let Some(newline) = pending.iter().position(|byte| *byte == b'\n') {
            let mut line = pending.drain(..=newline).collect::<Vec<_>>();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            if line.len() > MAX_FRAME_BYTES {
                return;
            }
            let frame = match serde_json::from_slice::<ClientFrame>(&line) {
                Ok(frame) => frame,
                Err(_) => return,
            };

            if !hello_complete {
                match frame {
                    ClientFrame::Hello { protocol } if protocol == PROTOCOL_VERSION => {
                        hello_complete = true;
                        if outgoing
                            .send(ServerFrame::Ready {
                                version: env!("CARGO_PKG_VERSION").to_owned(),
                            })
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                    _ => return,
                }
                continue;
            }

            match frame {
                ClientFrame::Hello { .. } => return,
                ClientFrame::Ping => {
                    if outgoing.send(ServerFrame::Pong).await.is_err() {
                        return;
                    }
                }
                ClientFrame::Cancel { .. } => {
                    // Cancellation is advisory in the protocol. The operation may
                    // finish and serve is free to ignore its reply.
                }
                ClientFrame::Request { id, request } => {
                    let outgoing = outgoing.clone();
                    let shared = shared.clone();
                    tokio::spawn(async move {
                        let result = match handle_request(shared, request).await {
                            Ok(reply) => ReplyResult::Ok(reply),
                            Err(error) => ReplyResult::Error(error),
                        };
                        let _ = outgoing.send(ServerFrame::Reply { id, result }).await;
                    });
                }
            }
        }
    }
}
