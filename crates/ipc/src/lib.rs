use llm_meter_core::IPC_VERSION;
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
#[cfg(unix)]
use std::path::PathBuf;
use std::{fmt, io, time::Duration};
use tokio::io::{
    AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader, ReadHalf,
    WriteHalf,
};

#[cfg(windows)]
const CONNECT_RETRY_INTERVAL: Duration = Duration::from_millis(40);
#[cfg(windows)]
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(2);
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IpcEndpoint {
    #[cfg(unix)]
    path: PathBuf,
    #[cfg(windows)]
    pipe_name: String,
}

impl IpcEndpoint {
    #[cfg(unix)]
    pub fn from_path(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    #[cfg(unix)]
    pub fn as_path(&self) -> &std::path::Path {
        &self.path
    }

    #[cfg(windows)]
    pub fn from_pipe_name(pipe_name: impl Into<String>) -> io::Result<Self> {
        let pipe_name = pipe_name.into();
        if !pipe_name.starts_with(r"\\.\pipe\") {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Windows IPC endpoint must be a local named pipe",
            ));
        }
        Ok(Self { pipe_name })
    }

    #[cfg(windows)]
    pub fn pipe_name(&self) -> &str {
        &self.pipe_name
    }
}

impl fmt::Display for IpcEndpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        #[cfg(unix)]
        return self.path.display().fmt(formatter);
        #[cfg(windows)]
        return formatter.write_str(&self.pipe_name);
    }
}

pub fn endpoint() -> io::Result<IpcEndpoint> {
    #[cfg(unix)]
    {
        let base = std::env::var_os("XDG_RUNTIME_DIR").ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "XDG_RUNTIME_DIR is required; refusing an unsafe /tmp fallback",
            )
        })?;
        let base = PathBuf::from(base);
        if !base.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "XDG_RUNTIME_DIR must be an absolute path",
            ));
        }
        Ok(IpcEndpoint::from_path(base.join("llm-meter/daemon.sock")))
    }
    #[cfg(windows)]
    {
        let scope = user_scope_id()?;
        IpcEndpoint::from_pipe_name(format!(r"\\.\pipe\io.github.llmmeter.v1.{scope}"))
    }
}

pub fn user_scope_id() -> io::Result<String> {
    #[cfg(windows)]
    let identity = windows::current_user_sid()?;
    #[cfg(unix)]
    let identity = std::env::var("UID")
        .or_else(|_| std::env::var("USER"))
        .map_err(|_| io::Error::new(io::ErrorKind::NotFound, "user identity is unavailable"))?;

    let digest = Sha256::digest(identity.as_bytes());
    Ok(hex_prefix(&digest, 12))
}

fn hex_prefix(bytes: &[u8], length: usize) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(length);
    for byte in bytes {
        if encoded.len() >= length {
            break;
        }
        encoded.push(HEX[(byte >> 4) as usize] as char);
        if encoded.len() < length {
            encoded.push(HEX[(byte & 0x0f) as usize] as char);
        }
    }
    encoded
}

#[cfg(unix)]
type ClientStream = tokio::net::UnixStream;
#[cfg(windows)]
type ClientStream = tokio::net::windows::named_pipe::NamedPipeClient;

pub struct Client {
    read: BufReader<ReadHalf<ClientStream>>,
    write: WriteHalf<ClientStream>,
    next_id: u64,
    #[cfg(windows)]
    server_process: windows::VerifiedServerProcess,
}

impl Client {
    pub async fn connect(endpoint: &IpcEndpoint) -> io::Result<Self> {
        let connected = connect_stream(endpoint).await?;
        let stream = connected.stream;
        let (read, write) = tokio::io::split(stream);
        let mut client = Self {
            read: BufReader::new(read),
            write,
            next_id: 0,
            #[cfg(windows)]
            server_process: connected.server_process,
        };
        let version =
            tokio::time::timeout(HANDSHAKE_TIMEOUT, client.call("system/version", json!({})))
                .await
                .map_err(|_| {
                    io::Error::new(io::ErrorKind::TimedOut, "daemon IPC handshake timed out")
                })??;
        if version.get("ipc_version").and_then(Value::as_u64) != Some(IPC_VERSION as u64) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "incompatible daemon IPC version",
            ));
        }
        Ok(client)
    }

    /// Returns the PID whose user SID was verified before the IPC handshake.
    #[cfg(windows)]
    pub fn server_pid(&self) -> u32 {
        self.server_process.pid()
    }

    /// Checks the verified server process object without blocking the caller.
    #[cfg(windows)]
    pub fn server_process_exited(&self) -> io::Result<bool> {
        self.server_process.has_exited()
    }

    pub async fn call(&mut self, method: &str, params: Value) -> io::Result<Value> {
        self.next_id += 1;
        let request = json!({
            "jsonrpc": "2.0",
            "id": self.next_id,
            "method": method,
            "params": params,
        });
        let encoded = encode_frame(&request, MAX_FRAME_BYTES)?;
        self.write.write_all(&encoded).await?;

        let response = read_frame(&mut self.read, MAX_FRAME_BYTES)
            .await?
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::UnexpectedEof, "daemon closed IPC connection")
            })?;
        let value: Value = serde_json::from_slice(&response).map_err(io::Error::other)?;
        if let Some(error) = value.get("error") {
            return Err(io::Error::other(error.to_string()));
        }
        Ok(value.get("result").cloned().unwrap_or(Value::Null))
    }
}

struct ConnectedStream {
    stream: ClientStream,
    #[cfg(windows)]
    server_process: windows::VerifiedServerProcess,
}

pub async fn call(endpoint: &IpcEndpoint, method: &str, params: Value) -> io::Result<Value> {
    Client::connect(endpoint).await?.call(method, params).await
}

#[cfg(unix)]
async fn connect_stream(endpoint: &IpcEndpoint) -> io::Result<ConnectedStream> {
    Ok(ConnectedStream {
        stream: tokio::net::UnixStream::connect(endpoint.as_path()).await?,
    })
}

#[cfg(windows)]
async fn connect_stream(endpoint: &IpcEndpoint) -> io::Result<ConnectedStream> {
    use tokio::net::windows::named_pipe::ClientOptions;
    use windows_sys::Win32::Foundation::ERROR_PIPE_BUSY;

    let open = async {
        loop {
            match ClientOptions::new().open(endpoint.pipe_name()) {
                Ok(client) => {
                    let server_process = windows::verify_server_identity(&client)?;
                    return Ok(ConnectedStream {
                        stream: client,
                        server_process,
                    });
                }
                Err(error) if error.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) => {
                    tokio::time::sleep(CONNECT_RETRY_INTERVAL).await;
                }
                Err(error) => return Err(error),
            }
        }
    };
    tokio::time::timeout(CONNECT_TIMEOUT, open)
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "daemon IPC connection timed out"))?
}

#[cfg(unix)]
pub type ServerConnection = tokio::net::UnixStream;
#[cfg(windows)]
pub type ServerConnection = tokio::net::windows::named_pipe::NamedPipeServer;

pub struct Listener {
    endpoint: IpcEndpoint,
    #[cfg(unix)]
    inner: tokio::net::UnixListener,
    #[cfg(windows)]
    next: Option<tokio::net::windows::named_pipe::NamedPipeServer>,
}

impl Listener {
    pub async fn bind(endpoint: &IpcEndpoint) -> io::Result<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::{FileTypeExt, PermissionsExt};

            let path = endpoint.as_path();
            let parent = path.parent().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "IPC socket has no parent")
            })?;
            tokio::fs::create_dir_all(parent).await?;
            tokio::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700)).await?;

            let inner = match tokio::net::UnixListener::bind(path) {
                Ok(listener) => listener,
                Err(bind_error) if bind_error.kind() == io::ErrorKind::AddrInUse => {
                    match tokio::net::UnixStream::connect(path).await {
                        Ok(_) => return Err(bind_error),
                        Err(_) => {
                            let metadata = std::fs::symlink_metadata(path)?;
                            if !metadata.file_type().is_socket() {
                                return Err(io::Error::new(
                                    io::ErrorKind::AddrInUse,
                                    "refusing to replace a non-socket IPC path",
                                ));
                            }
                            tokio::fs::remove_file(path).await?;
                            tokio::net::UnixListener::bind(path)?
                        }
                    }
                }
                Err(error) => return Err(error),
            };
            tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).await?;
            Ok(Self {
                endpoint: endpoint.clone(),
                inner,
            })
        }
        #[cfg(windows)]
        {
            let next = windows::create_pipe(endpoint, true)?;
            Ok(Self {
                endpoint: endpoint.clone(),
                next: Some(next),
            })
        }
    }

    pub async fn accept(&mut self) -> io::Result<ServerConnection> {
        #[cfg(unix)]
        {
            self.inner.accept().await.map(|(stream, _)| stream)
        }
        #[cfg(windows)]
        {
            loop {
                let connected = self
                    .next
                    .take()
                    .expect("named pipe listener always keeps a pending instance");
                connected.connect().await?;
                self.next = Some(windows::create_pipe(&self.endpoint, false)?);
                if windows::verify_client_identity(&connected).is_ok() {
                    return Ok(connected);
                }
            }
        }
    }
}

#[cfg(unix)]
impl Drop for Listener {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(self.endpoint.as_path());
    }
}

pub async fn serve_connection<S, F, Fut>(stream: S, mut handler: F) -> io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
    F: FnMut(Value) -> Fut,
    Fut: std::future::Future<Output = Value>,
{
    let (read, mut write) = tokio::io::split(stream);
    let mut read = BufReader::new(read);
    loop {
        let Some(request) = read_frame(&mut read, MAX_FRAME_BYTES).await? else {
            return Ok(());
        };
        let response =
            handler(serde_json::from_slice(&request).unwrap_or_else(
                |_| json!({"jsonrpc":"2.0","id":null,"_transport_parse_error":true}),
            ))
            .await;
        let encoded = encode_frame(&response, MAX_FRAME_BYTES)?;
        write.write_all(&encoded).await?;
    }
}

pub fn encode_frame<T>(value: &T, limit: usize) -> io::Result<Vec<u8>>
where
    T: Serialize + ?Sized,
{
    let mut encoded = serde_json::to_vec(value).map_err(io::Error::other)?;
    if encoded.len().saturating_add(1) > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "IPC frame exceeded the size limit",
        ));
    }
    encoded.push(b'\n');
    Ok(encoded)
}

pub async fn read_frame<R>(reader: &mut R, limit: usize) -> io::Result<Option<Vec<u8>>>
where
    R: AsyncBufRead + Unpin,
{
    let mut frame = Vec::new();
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return if frame.is_empty() {
                Ok(None)
            } else {
                Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "IPC frame ended before a newline delimiter",
                ))
            };
        }
        let count = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |position| position + 1);
        if frame.len().saturating_add(count) > limit {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "IPC frame exceeded the size limit",
            ));
        }
        let complete = available[count - 1] == b'\n';
        frame.extend_from_slice(&available[..count]);
        reader.consume(count);
        if complete {
            return Ok(Some(frame));
        }
    }
}

#[cfg(windows)]
mod windows {
    use super::{IpcEndpoint, io};
    use std::{
        ffi::c_void,
        mem,
        os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
        ptr,
    };
    use tokio::net::windows::named_pipe::{NamedPipeClient, NamedPipeServer, ServerOptions};
    use windows_sys::Win32::{
        Foundation::{
            CloseHandle, ERROR_INSUFFICIENT_BUFFER, HANDLE, LocalFree, WAIT_FAILED, WAIT_OBJECT_0,
            WAIT_TIMEOUT,
        },
        Security::{
            Authorization::{
                ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
                SDDL_REVISION_1,
            },
            GetTokenInformation, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser,
        },
        System::{
            Pipes::{GetNamedPipeClientProcessId, GetNamedPipeServerProcessId},
            Threading::{
                GetCurrentProcess, OpenProcess, OpenProcessToken,
                PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, WaitForSingleObject,
            },
        },
    };

    pub(super) struct VerifiedServerProcess {
        pid: u32,
        handle: OwnedHandle,
    }

    impl VerifiedServerProcess {
        fn open(pid: u32) -> io::Result<Self> {
            let handle = unsafe {
                OpenProcess(
                    PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                    0,
                    pid,
                )
            };
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            Ok(Self {
                pid,
                handle: unsafe { OwnedHandle::from_raw_handle(handle) },
            })
        }

        pub(super) fn pid(&self) -> u32 {
            self.pid
        }

        pub(super) fn has_exited(&self) -> io::Result<bool> {
            let result = unsafe { WaitForSingleObject(self.handle.as_raw_handle(), 0) };
            match result {
                WAIT_OBJECT_0 => Ok(true),
                WAIT_TIMEOUT => Ok(false),
                WAIT_FAILED => Err(io::Error::last_os_error()),
                other => Err(io::Error::other(format!(
                    "unexpected process wait result: 0x{other:08x}"
                ))),
            }
        }
    }

    pub(super) fn current_user_sid() -> io::Result<String> {
        sid_for_process(unsafe { GetCurrentProcess() })
    }

    fn sid_for_process(process: HANDLE) -> io::Result<String> {
        let mut token = ptr::null_mut();
        if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let result = sid_for_token(token);
        unsafe {
            CloseHandle(token);
        }
        result
    }

    fn sid_for_token(token: HANDLE) -> io::Result<String> {
        let mut required = 0;
        unsafe {
            GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut required);
        }
        if required == 0
            || io::Error::last_os_error().raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER as i32)
        {
            return Err(io::Error::last_os_error());
        }
        let word_size = mem::size_of::<usize>();
        let word_count = (required as usize).div_ceil(word_size);
        let mut buffer = vec![0_usize; word_count];
        let buffer_bytes = buffer
            .len()
            .checked_mul(word_size)
            .and_then(|length| u32::try_from(length).ok())
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "token information is too large")
            })?;
        if unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                buffer.as_mut_ptr().cast(),
                buffer_bytes,
                &mut required,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        if (required as usize) < mem::size_of::<TOKEN_USER>() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "token user information is truncated",
            ));
        }
        let user = unsafe { &*(buffer.as_ptr().cast::<TOKEN_USER>()) };
        let mut encoded = ptr::null_mut();
        if unsafe { ConvertSidToStringSidW(user.User.Sid, &mut encoded) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut length = 0;
        while unsafe { *encoded.add(length) } != 0 {
            length += 1;
        }
        let sid = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(encoded, length) });
        unsafe {
            LocalFree(encoded.cast());
        }
        Ok(sid)
    }

    fn sid_for_pid(pid: u32) -> io::Result<String> {
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if process.is_null() {
            return Err(io::Error::last_os_error());
        }
        let result = sid_for_process(process);
        unsafe {
            CloseHandle(process);
        }
        result
    }

    pub(super) fn create_pipe(endpoint: &IpcEndpoint, first: bool) -> io::Result<NamedPipeServer> {
        let sid = current_user_sid()?;
        let sddl = format!("D:P(A;;GA;;;SY)(A;;GA;;;{sid})");
        let mut wide = sddl.encode_utf16().collect::<Vec<_>>();
        wide.push(0);
        let mut descriptor: *mut c_void = ptr::null_mut();
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                wide.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                ptr::null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        let mut attributes = SECURITY_ATTRIBUTES {
            nLength: mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor,
            bInheritHandle: 0,
        };
        let result = unsafe {
            ServerOptions::new()
                .first_pipe_instance(first)
                .reject_remote_clients(true)
                .create_with_security_attributes_raw(
                    endpoint.pipe_name(),
                    (&mut attributes as *mut SECURITY_ATTRIBUTES).cast(),
                )
        };
        unsafe {
            LocalFree(descriptor);
        }
        result
    }

    pub(super) fn verify_server_identity(
        client: &NamedPipeClient,
    ) -> io::Result<VerifiedServerProcess> {
        let mut pid = 0;
        if unsafe { GetNamedPipeServerProcessId(client.as_raw_handle().cast(), &mut pid) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let process = VerifiedServerProcess::open(pid)?;
        verify_process_user(
            process.handle.as_raw_handle(),
            "named pipe server belongs to a different user",
        )?;
        Ok(process)
    }

    pub(super) fn verify_client_identity(server: &NamedPipeServer) -> io::Result<()> {
        let mut pid = 0;
        if unsafe { GetNamedPipeClientProcessId(server.as_raw_handle().cast(), &mut pid) } == 0 {
            return Err(io::Error::last_os_error());
        }
        verify_same_user(pid, "named pipe client belongs to a different user")
    }

    fn verify_same_user(pid: u32, message: &'static str) -> io::Result<()> {
        if sid_for_pid(pid)? == current_user_sid()? {
            Ok(())
        } else {
            Err(io::Error::new(io::ErrorKind::PermissionDenied, message))
        }
    }

    fn verify_process_user(process: HANDLE, message: &'static str) -> io::Result<()> {
        if sid_for_process(process)? == current_user_sid()? {
            Ok(())
        } else {
            Err(io::Error::new(io::ErrorKind::PermissionDenied, message))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_hash_is_stable_and_opaque() {
        assert_eq!(hex_prefix(&Sha256::digest(b"S-1-5-21-test"), 12).len(), 12);
        assert_eq!(
            hex_prefix(&Sha256::digest(b"S-1-5-21-test"), 12),
            hex_prefix(&Sha256::digest(b"S-1-5-21-test"), 12)
        );
    }

    #[tokio::test]
    async fn frame_reader_enforces_delimiters_and_limits() {
        let mut complete = BufReader::new(&b"one\ntwo\n"[..]);
        assert_eq!(
            read_frame(&mut complete, 8).await.unwrap().unwrap(),
            b"one\n"
        );
        assert_eq!(
            read_frame(&mut complete, 8).await.unwrap().unwrap(),
            b"two\n"
        );
        assert!(read_frame(&mut complete, 8).await.unwrap().is_none());

        let mut oversized = BufReader::new(&b"123456789\n"[..]);
        assert_eq!(
            read_frame(&mut oversized, 8).await.unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );

        let mut partial = BufReader::new(&b"partial"[..]);
        assert_eq!(
            read_frame(&mut partial, 8).await.unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
    }

    #[test]
    fn frame_writer_enforces_the_same_limit_as_the_reader() {
        assert_eq!(
            encode_frame(&json!({"ok": true}), 32).unwrap().last(),
            Some(&b'\n')
        );
        assert_eq!(
            encode_frame(&json!({"value": "x".repeat(64)}), 32)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[cfg(windows)]
    #[test]
    fn default_endpoint_is_a_scoped_local_pipe() {
        let endpoint = endpoint().unwrap();
        assert!(
            endpoint
                .pipe_name()
                .starts_with(r"\\.\pipe\io.github.llmmeter.v1.")
        );
        assert!(!endpoint.pipe_name().contains("S-1-"));
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn client_retains_the_verified_server_process() {
        use std::sync::atomic::{AtomicU64, Ordering};

        static NEXT_PIPE: AtomicU64 = AtomicU64::new(1);
        let endpoint = IpcEndpoint::from_pipe_name(format!(
            r"\\.\pipe\llm-meter-ipc-test-{}-{}",
            std::process::id(),
            NEXT_PIPE.fetch_add(1, Ordering::Relaxed)
        ))
        .unwrap();
        let mut listener = Listener::bind(&endpoint).await.unwrap();
        let server = tokio::spawn(async move {
            let stream = listener.accept().await.unwrap();
            serve_connection(stream, |request| async move {
                json!({
                    "jsonrpc": "2.0",
                    "id": request.get("id").cloned().unwrap_or(Value::Null),
                    "result": { "ipc_version": IPC_VERSION },
                })
            })
            .await
            .unwrap();
        });

        let client = Client::connect(&endpoint).await.unwrap();
        assert_eq!(client.server_pid(), std::process::id());
        assert!(!client.server_process_exited().unwrap());
        drop(client);
        server.await.unwrap();
    }
}
