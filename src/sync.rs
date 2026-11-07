use crate::{
    model::Record,
    store::{self, Result},
    transfer::{self, EntrySummary},
};
use bincode::Options;
use rustls::{
    ClientConfig, ClientConnection, ServerConfig, ServerConnection, StreamOwned,
    pki_types::{CertificateDer, PrivatePkcs8KeyDer, ServerName},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs,
    io::{Read, Write},
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream, UdpSocket},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread,
    time::{Duration, Instant},
};
const UDP: u16 = 39471;
const TCP: u16 = 39472;
const FRAME: u64 = 16 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(5);
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}
#[derive(Clone)]
pub struct SyncHandle {
    shared: Arc<Shared>,
}
#[derive(Clone)]
pub struct PeerHistory {
    pub id: String,
    pub name: String,
    pub online: bool,
    pub entries: Vec<EntrySummary>,
}
pub enum SyncEvent {
    Peers(Vec<PeerHistory>),
    Status(String),
}
#[derive(Clone)]
struct Peer {
    name: String,
    cert: Vec<u8>,
    addr: SocketAddr,
    entries: Vec<EntrySummary>,
    seen: Instant,
    online: bool,
    polling: bool,
    last_poll: Option<Instant>,
}
struct Shared {
    root: PathBuf,
    name: String,
    enabled: AtomicBool,
    launched: AtomicBool,
    summaries: Mutex<Vec<EntrySummary>>,
    peers: Mutex<HashMap<String, Peer>>,
    events: SyncSender<SyncEvent>,
    config: Config,
    active_server: AtomicUsize,
    active_poll: AtomicUsize,
    restoring: Mutex<()>,
}
#[derive(Clone)]
struct Config {
    udp: u16,
    tcp: u16,
    targets: Vec<SocketAddr>,
    test: bool,
}
#[derive(Serialize, Deserialize)]
struct Discovery {
    version: u8,
    name: String,
    cert: Vec<u8>,
    tcp: u16,
    reply: bool,
}
#[derive(Serialize, Deserialize)]
enum Request {
    Index,
    Entry(String),
}
#[derive(Serialize, Deserialize)]
struct Identity {
    cert: Vec<u8>,
    key: Vec<u8>,
}

pub fn start(root: PathBuf, enabled_by_default: bool) -> Result<(SyncHandle, Receiver<SyncEvent>)> {
    start_config(
        root,
        enabled_by_default,
        Config {
            udp: UDP,
            tcp: TCP,
            targets: vec![],
            test: false,
        },
    )
}
fn start_config(
    root: PathBuf,
    enabled_by_default: bool,
    config: Config,
) -> Result<(SyncHandle, Receiver<SyncEvent>)> {
    fs::create_dir_all(&root).map_err(err)?;
    let enabled = match fs::read(root.join("sync-enabled")) {
        Ok(v) if v == b"1" => true,
        Ok(v) if v == b"0" => false,
        Ok(_) => return Err("Invalid sync setting".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => enabled_by_default,
        Err(e) => return Err(err(e)),
    };
    let (events, receiver) = mpsc::sync_channel(8);
    let shared = Arc::new(Shared {
        root,
        name: std::env::var("COMPUTERNAME")
            .unwrap_or_else(|_| "PC".into())
            .chars()
            .take(80)
            .collect(),
        enabled: AtomicBool::new(enabled),
        launched: AtomicBool::new(false),
        summaries: Mutex::new(vec![]),
        peers: Mutex::new(HashMap::new()),
        events,
        config,
        active_server: AtomicUsize::new(0),
        active_poll: AtomicUsize::new(0),
        restoring: Mutex::new(()),
    });
    let handle = SyncHandle { shared };
    if enabled {
        launch(&handle.shared);
    }
    Ok((handle, receiver))
}
impl SyncHandle {
    pub fn local_name(&self) -> String {
        self.shared.name.clone()
    }
    pub fn publish(&self, entries: Vec<EntrySummary>) {
        *self.shared.summaries.lock().unwrap() = entries.into_iter().take(100).collect();
    }
    pub fn is_enabled(&self) -> bool {
        self.shared.enabled.load(Ordering::SeqCst)
    }
    pub fn shutdown(&self) {
        self.shared.enabled.store(false, Ordering::SeqCst);
    }
    pub fn set_enabled(&self, enabled: bool) -> Result<()> {
        let path = self.shared.root.join("sync-enabled");
        if !enabled {
            self.shared.enabled.store(false, Ordering::SeqCst);
        }
        let persisted = fs::write(&path, if enabled { b"1" } else { b"0" });
        if enabled && persisted.is_ok() {
            self.shared.enabled.store(true, Ordering::SeqCst);
        }
        if enabled && persisted.is_ok() {
            launch(&self.shared);
        } else {
            let mut peers = self.shared.peers.lock().unwrap();
            for peer in peers.values_mut() {
                peer.online = false;
            }
            drop(peers);
            emit_peers(&self.shared);
        }
        persisted.map_err(|e| format!("Could not persist sync setting: {e}"))?;
        Ok(())
    }
    pub fn restore(&self, peer_id: &str, entry_id: &str) -> Result<(Record, Vec<PathBuf>)> {
        if !self.is_enabled() {
            return Err("Sync is disabled".into());
        }
        if !safe_id(peer_id) || !safe_id(entry_id) {
            return Err("Invalid peer or entry ID".into());
        }
        let peer = self
            .shared
            .peers
            .lock()
            .unwrap()
            .get(peer_id)
            .cloned()
            .ok_or("Peer unavailable")?;
        if !peer.online || !peer.entries.iter().any(|e| e.id == entry_id) {
            return Err("Entry unavailable".into());
        }
        let _restore = self.shared.restoring.lock().unwrap();
        if !self.is_enabled() {
            return Err("Sync is disabled".into());
        }
        let deadline = Instant::now() + Duration::from_secs(120);
        let mut stream = connect(&peer, &self.shared.enabled, deadline)?;
        write_frame(&mut stream, &Request::Entry(entry_id.into()))?;
        let ok: bool = read_frame(&mut stream, 16)?;
        if !ok || !self.is_enabled() {
            return Err("Remote entry unavailable".into());
        }
        transfer::read_entry(
            &self.shared.root.join("peers").join(peer_id),
            entry_id,
            &mut stream,
        )
    }
}
fn safe_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= 128 && s.bytes().all(|x| x.is_ascii_hexdigit())
}
fn launch(shared: &Arc<Shared>) {
    if shared.launched.swap(true, Ordering::SeqCst) {
        return;
    }
    let s = shared.clone();
    thread::spawn(move || match identity(&s.root) {
        Ok(id) => {
            let id = Arc::new(id);
            let s2 = s.clone();
            let i2 = id.clone();
            thread::spawn(move || server(s2, i2));
            discovery(s.clone(), id);
        }
        Err(e) => {
            let _ = s
                .events
                .try_send(SyncEvent::Status(format!("Sync unavailable: {e}")));
            s.launched.store(false, Ordering::SeqCst);
        }
    });
}
fn identity(root: &Path) -> Result<Identity> {
    let path = root.join("sync-identity");
    match fs::read(&path) {
        Ok(bytes) => bincode::deserialize(&store::unprotect(&bytes)?).map_err(err),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let generated =
                rcgen::generate_simple_self_signed(vec!["clipvault.local".into()]).map_err(err)?;
            let id = Identity {
                cert: generated.cert.der().to_vec(),
                key: generated.signing_key.serialize_der(),
            };
            let data = store::protect(&bincode::serialize(&id).map_err(err)?)?;
            fs::write(&path, data).map_err(err)?;
            Ok(id)
        }
        Err(e) => Err(err(e)),
    }
}
fn id_for(cert: &[u8]) -> String {
    Sha256::digest(cert)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn allowed(ip: Ipv4Addr) -> bool {
    let [a, b, _, _] = ip.octets();
    a == 127
        || a == 10
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && b == 168)
        || (a == 169 && b == 254)
        || (a == 100 && (64..=127).contains(&b))
}
fn tailscale_addresses(json: &str) -> Vec<Ipv4Addr> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        return vec![];
    };
    value
        .get("Peer")
        .and_then(|p| p.as_object())
        .into_iter()
        .flat_map(|p| p.values())
        .filter(|p| p.get("Online").and_then(|v| v.as_bool()) == Some(true))
        .flat_map(|p| {
            p.get("TailscaleIPs")
                .and_then(|v| v.as_array())
                .into_iter()
                .flatten()
        })
        .filter_map(|v| v.as_str()?.parse::<Ipv4Addr>().ok())
        .filter(|ip| {
            let [a, b, _, _] = ip.octets();
            a == 100 && (64..=127).contains(&b)
        })
        .collect()
}
fn tailscale() -> Vec<Ipv4Addr> {
    use std::os::windows::process::CommandExt;
    let Ok(mut child) = Command::new("tailscale")
        .args(["status", "--json"])
        .creation_flags(0x08000000)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return vec![];
    };
    let out = child.stdout.take().unwrap();
    let reader = thread::spawn(move || {
        let mut b = vec![];
        let _ = out.take(4 * 1024 * 1024).read_to_end(&mut b);
        b
    });
    let until = Instant::now() + Duration::from_secs(3);
    while Instant::now() < until {
        if child.try_wait().ok().flatten().is_some() {
            return tailscale_addresses(&String::from_utf8_lossy(
                &reader.join().unwrap_or_default(),
            ));
        }
        thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
    let _ = reader.join();
    vec![]
}
fn broadcasts() -> Vec<Ipv4Addr> {
    use windows_sys::Win32::NetworkManagement::IpHelper::{GetIpAddrTable, MIB_IPADDRTABLE};
    let mut size = 0u32;
    unsafe {
        GetIpAddrTable(std::ptr::null_mut(), &mut size, 0);
    }
    if size == 0 || size > 1024 * 1024 {
        return vec![];
    }
    let mut buf = vec![0u64; (size as usize).div_ceil(8)];
    let table = buf.as_mut_ptr().cast::<MIB_IPADDRTABLE>();
    if unsafe { GetIpAddrTable(table, &mut size, 0) } != 0 {
        return vec![];
    }
    let rows = unsafe {
        std::slice::from_raw_parts((*table).table.as_ptr(), (*table).dwNumEntries as usize)
    };
    rows.iter()
        .filter_map(|r| {
            let ip = Ipv4Addr::from(r.dwAddr.to_ne_bytes());
            if !allowed(ip) || ip.is_loopback() || r.dwMask == u32::MAX {
                None
            } else {
                Some(Ipv4Addr::from((r.dwAddr | !r.dwMask).to_ne_bytes()))
            }
        })
        .collect()
}
fn write_frame<T: Serialize>(out: &mut impl Write, value: &T) -> Result<()> {
    let bytes = bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .with_limit(FRAME)
        .serialize(value)
        .map_err(err)?;
    out.write_all(&(bytes.len() as u32).to_le_bytes())
        .map_err(err)?;
    out.write_all(&bytes).map_err(err)?;
    out.flush().map_err(err)
}
fn read_frame<T: DeserializeOwned>(input: &mut impl Read, limit: u64) -> Result<T> {
    let mut size = [0u8; 4];
    input.read_exact(&mut size).map_err(err)?;
    let len = u32::from_le_bytes(size) as u64;
    if len > limit {
        return Err("Oversized network frame".into());
    }
    let mut bytes = vec![0; len as usize];
    input.read_exact(&mut bytes).map_err(err)?;
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .with_limit(limit)
        .reject_trailing_bytes()
        .deserialize(&bytes)
        .map_err(err)
}
struct SocketGuard<'a> {
    inner: TcpStream,
    enabled: &'a AtomicBool,
    deadline: Instant,
}
impl SocketGuard<'_> {
    fn timeout(&self, idle_deadline: Instant) -> std::io::Result<Duration> {
        if !self.enabled.load(Ordering::SeqCst) {
            return Err(std::io::Error::other("Sync disabled"));
        }
        let remaining = self
            .deadline
            .min(idle_deadline)
            .checked_duration_since(Instant::now())
            .filter(|v| !v.is_zero())
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::TimedOut, "Transfer timed out")
            })?;
        Ok(remaining.min(Duration::from_millis(200)))
    }
    fn retryable(e: &std::io::Error) -> bool {
        matches!(
            e.kind(),
            std::io::ErrorKind::WouldBlock
                | std::io::ErrorKind::TimedOut
                | std::io::ErrorKind::Interrupted
        )
    }
}
impl Read for SocketGuard<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let idle = Instant::now() + TIMEOUT;
        loop {
            self.inner.set_read_timeout(Some(self.timeout(idle)?))?;
            match self.inner.read(buf) {
                Err(e) if Self::retryable(&e) => continue,
                result => return result,
            }
        }
    }
}
impl Write for SocketGuard<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let idle = Instant::now() + TIMEOUT;
        loop {
            self.inner.set_write_timeout(Some(self.timeout(idle)?))?;
            match self.inner.write(buf) {
                Err(e) if Self::retryable(&e) => continue,
                result => return result,
            }
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.timeout(Instant::now() + TIMEOUT)?;
        self.inner.flush()
    }
}
fn connect<'a>(
    peer: &Peer,
    enabled: &'a AtomicBool,
    deadline: Instant,
) -> Result<StreamOwned<ClientConnection, SocketGuard<'a>>> {
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(CertificateDer::from(peer.cert.clone()))
        .map_err(err)?;
    let config = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let conn = ClientConnection::new(
        Arc::new(config),
        ServerName::try_from("clipvault.local")
            .map_err(err)?
            .to_owned(),
    )
    .map_err(err)?;
    if !enabled.load(Ordering::SeqCst) {
        return Err("Sync disabled".into());
    }
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .filter(|d| !d.is_zero())
        .ok_or("Transfer timed out")?;
    let socket = TcpStream::connect_timeout(&peer.addr, remaining.min(TIMEOUT)).map_err(err)?;
    socket.set_nonblocking(false).map_err(err)?;
    let mut stream = StreamOwned::new(
        conn,
        SocketGuard {
            inner: socket,
            enabled,
            deadline,
        },
    );
    while stream.conn.is_handshaking() {
        stream.conn.complete_io(&mut stream.sock).map_err(err)?;
    }
    if stream
        .conn
        .peer_certificates()
        .and_then(|v| v.first())
        .is_none_or(|c| c.as_ref() != peer.cert)
    {
        return Err("Peer certificate mismatch".into());
    }
    Ok(stream)
}
fn server(shared: Arc<Shared>, identity: Arc<Identity>) {
    let cert = CertificateDer::from(identity.cert.clone());
    let key = PrivatePkcs8KeyDer::from(identity.key.clone());
    let Ok(config) = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert], key.into())
    else {
        let _ = shared
            .events
            .try_send(SyncEvent::Status("Sync TLS configuration failed".into()));
        return;
    };
    let config = Arc::new(config);
    let listener = match TcpListener::bind((Ipv4Addr::UNSPECIFIED, shared.config.tcp)) {
        Ok(v) => v,
        Err(e) => {
            let _ = shared
                .events
                .try_send(SyncEvent::Status(format!("Sync server unavailable: {e}")));
            return;
        }
    };
    let _ = listener.set_nonblocking(true);
    loop {
        if !shared.enabled.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_millis(200));
            continue;
        }
        match listener.accept() {
            Ok((stream, addr)) if matches!(addr.ip(),IpAddr::V4(ip) if allowed(ip)) => {
                if shared.active_server.fetch_add(1, Ordering::SeqCst) >= 8 {
                    shared.active_server.fetch_sub(1, Ordering::SeqCst);
                    continue;
                }
                let s = shared.clone();
                let c = config.clone();
                thread::spawn(move || {
                    let _ = serve(&s, c, stream);
                    s.active_server.fetch_sub(1, Ordering::SeqCst);
                });
            }
            _ => thread::sleep(Duration::from_millis(30)),
        }
    }
}
fn serve(shared: &Shared, config: Arc<ServerConfig>, socket: TcpStream) -> Result<()> {
    socket.set_nonblocking(false).map_err(err)?;
    let conn = ServerConnection::new(config).map_err(err)?;
    let guarded = SocketGuard {
        inner: socket,
        enabled: &shared.enabled,
        deadline: Instant::now() + Duration::from_secs(120),
    };
    let mut stream = StreamOwned::new(conn, guarded);
    let request: Request = read_frame(&mut stream, 1024)?;
    if !shared.enabled.load(Ordering::SeqCst) {
        return Err("Sync disabled".into());
    }
    match request {
        Request::Index => write_frame(&mut stream, &*shared.summaries.lock().unwrap()),
        Request::Entry(id) => {
            if !safe_id(&id) || !shared.summaries.lock().unwrap().iter().any(|e| e.id == id) {
                return write_frame(&mut stream, &false);
            }
            write_frame(&mut stream, &true)?;
            transfer::write_entry(&shared.root, &id, &mut stream)?;
            stream.flush().map_err(err)
        }
    }
}
fn emit_peers(shared: &Shared) {
    let mut peers: Vec<_> = shared
        .peers
        .lock()
        .unwrap()
        .iter()
        .map(|(id, p)| PeerHistory {
            id: id.clone(),
            name: p.name.clone(),
            online: p.online,
            entries: p.entries.clone(),
        })
        .collect();
    peers.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
    let _ = shared.events.try_send(SyncEvent::Peers(peers));
}
fn discovery(shared: Arc<Shared>, identity: Arc<Identity>) {
    let socket = match UdpSocket::bind((Ipv4Addr::UNSPECIFIED, shared.config.udp)) {
        Ok(s) => s,
        Err(e) => {
            let _ = shared.events.try_send(SyncEvent::Status(format!(
                "Sync discovery unavailable: {e}"
            )));
            return;
        }
    };
    let _ = socket.set_read_timeout(Some(Duration::from_millis(200)));
    let _ = socket.set_broadcast(true);
    let mut next = Instant::now();
    let mut targets = vec![];
    let mut refresh = Instant::now();
    let own = id_for(&identity.cert);
    let mut buf = [0u8; 8192];
    let mut rate_window = Instant::now();
    let mut packet_count = 0u32;
    let mut new_window = Instant::now();
    let mut new_count = 0u32;
    loop {
        if !shared.enabled.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_millis(200));
            continue;
        }
        if refresh <= Instant::now() {
            targets = shared.config.targets.clone();
            if !shared.config.test {
                targets.extend(
                    broadcasts()
                        .into_iter()
                        .chain(tailscale())
                        .map(|ip| SocketAddr::from((ip, shared.config.udp))),
                );
            }
            refresh = Instant::now() + Duration::from_secs(30);
        }
        if next <= Instant::now() {
            let d = Discovery {
                version: 1,
                name: shared.name.clone(),
                cert: identity.cert.clone(),
                tcp: shared.config.tcp,
                reply: false,
            };
            if let Ok(data) = bincode::serialize(&d) {
                for target in &targets {
                    let _ = socket.send_to(&data, target);
                }
            }
            {
                let mut peers = shared.peers.lock().unwrap();
                peers.retain(|_, p| p.seen.elapsed() < Duration::from_secs(600));
                for p in peers.values_mut() {
                    if p.seen.elapsed() > Duration::from_secs(20) {
                        p.online = false;
                    }
                }
            }
            emit_peers(&shared);
            next = Instant::now() + Duration::from_secs(3);
        }
        if let Ok((n, source)) = socket.recv_from(&mut buf) {
            if rate_window.elapsed() >= Duration::from_secs(1) {
                rate_window = Instant::now();
                packet_count = 0;
            }
            packet_count += 1;
            if packet_count > 100 {
                continue;
            }
            if !matches!(source.ip(), IpAddr::V4(ip) if allowed(ip)) {
                continue;
            }
            let Ok(d) = bincode::DefaultOptions::new()
                .with_fixint_encoding()
                .with_limit(8192)
                .reject_trailing_bytes()
                .deserialize::<Discovery>(&buf[..n])
            else {
                continue;
            };
            if d.version != 1
                || d.cert.is_empty()
                || d.cert.len() > 4096
                || d.name.len() > 240
                || d.tcp == 0
            {
                continue;
            }
            let id = id_for(&d.cert);
            if id == own {
                continue;
            }
            let addr = SocketAddr::new(source.ip(), d.tcp);
            let mut peers = shared.peers.lock().unwrap();
            if new_window.elapsed() >= Duration::from_secs(60) {
                new_window = Instant::now();
                new_count = 0;
            }
            if !peers.contains_key(&id) {
                if new_count >= 30 {
                    continue;
                }
                new_count += 1;
            }
            let p = peers.entry(id.clone()).or_insert(Peer {
                name: d.name.clone(),
                cert: d.cert.clone(),
                addr,
                entries: vec![],
                seen: Instant::now(),
                online: false,
                polling: false,
                last_poll: None,
            });
            p.name = d.name;
            p.addr = addr;
            p.seen = Instant::now();
            let due = !p.polling
                && p.last_poll
                    .is_none_or(|t| t.elapsed() >= Duration::from_secs(3));
            if due && shared.active_poll.fetch_add(1, Ordering::SeqCst) < 8 {
                p.last_poll = Some(Instant::now());
                p.polling = true;
                let peer = p.clone();
                let s = shared.clone();
                thread::spawn(move || {
                    let deadline = Instant::now() + TIMEOUT;
                    let result = connect(&peer, &s.enabled, deadline).and_then(|mut stream| {
                        write_frame(&mut stream, &Request::Index)?;
                        let entries: Vec<EntrySummary> = read_frame(&mut stream, FRAME)?;
                        if entries.len() > 100
                            || entries.iter().any(|e| {
                                !safe_id(&e.id)
                                    || e.title.len() > 65536
                                    || e.detail.len() > 65536
                                    || e.preview.as_ref().is_some_and(|p| {
                                        p.width == 0
                                            || p.height == 0
                                            || p.width > 80
                                            || p.height > 80
                                            || (p.width as u64)
                                                .checked_mul(p.height as u64)
                                                .and_then(|v| v.checked_mul(4))
                                                != Some(p.rgba.len() as u64)
                                    })
                            })
                        {
                            return Err("Invalid peer index".into());
                        }
                        Ok(entries)
                    });
                    let mut peers = s.peers.lock().unwrap();
                    // Keep metadata memory bounded without a fixed peer-count limit.
                    let used: u64 = peers
                        .iter()
                        .filter(|(key, _)| *key != &id)
                        .map(|(_, peer)| bincode::serialized_size(&peer.entries).unwrap_or(FRAME))
                        .sum();
                    if let Some(p) = peers.get_mut(&id) {
                        p.polling = false;
                        p.online = result.is_ok() && s.enabled.load(Ordering::SeqCst);
                        if let Ok(entries) = result
                            && used
                                .saturating_add(bincode::serialized_size(&entries).unwrap_or(FRAME))
                                <= 64 * 1024 * 1024
                        {
                            p.entries = entries;
                        }
                    }
                    drop(peers);
                    s.active_poll.fetch_sub(1, Ordering::SeqCst);
                    emit_peers(&s);
                });
            } else if due {
                shared.active_poll.fetch_sub(1, Ordering::SeqCst);
            }
            drop(peers);
            if !d.reply {
                let d = Discovery {
                    version: 1,
                    name: shared.name.clone(),
                    cert: identity.cert.clone(),
                    tcp: shared.config.tcp,
                    reply: true,
                };
                if let Ok(data) = bincode::serialize(&d) {
                    let _ = socket.send_to(&data, source);
                }
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shutdown_cancels_sharing_without_changing_the_saved_preference() {
        let dir = tempfile::tempdir().unwrap();
        let (handle, _) = start_config(
            dir.path().into(),
            false,
            Config {
                udp: port(),
                tcp: port(),
                targets: vec![],
                test: true,
            },
        )
        .unwrap();
        fs::write(dir.path().join("sync-enabled"), b"1").unwrap();
        handle.shared.enabled.store(true, Ordering::SeqCst);
        handle.shutdown();
        assert!(!handle.is_enabled());
        assert_eq!(fs::read(dir.path().join("sync-enabled")).unwrap(), b"1");
    }
    #[test]
    fn address_policy() {
        for address in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.2",
            "169.254.1.2",
            "100.64.0.1",
            "100.127.255.255",
        ] {
            assert!(allowed(address.parse().unwrap()), "{address}");
        }
        for address in [
            "8.8.8.8",
            "172.32.0.1",
            "100.128.0.1",
            "0.0.0.0",
            "224.0.0.1",
        ] {
            assert!(!allowed(address.parse().unwrap()), "{address}");
        }
    }
    #[test]
    fn tailscale_only_online_allowed_ipv4() {
        let json = r#"{"Peer":{"a":{"Online":true,"TailscaleIPs":["100.64.0.2","2001:db8::1","8.8.8.8"]},"b":{"Online":false,"TailscaleIPs":["100.64.0.3"]}}}"#;
        assert_eq!(
            tailscale_addresses(json),
            vec![Ipv4Addr::new(100, 64, 0, 2)]
        );
    }
    fn port() -> u16 {
        TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }
    #[test]
    fn loopback_discovery_restore_and_disable() {
        use crate::{
            model::{Kind, Record},
            store::Store,
        };
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let mut source = Store::open(a.path().into()).unwrap();
        source.insert(Record::text("network text"), &[]).unwrap();
        let mut image = Record::text("network image");
        image.kind = Kind::Image;
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgba8(2, 2)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        let png = png.into_inner();
        image.formats.push(crate::model::Format {
            id: 8,
            name: None,
            data: png.clone(),
        });
        source.insert(image, &[]).unwrap();
        let file = a.path().join("source.bin");
        fs::write(&file, [1, 3, 5]).unwrap();
        let mut record = Record::text("network file");
        record.kind = Kind::Files;
        source.insert(record, &[file]).unwrap();
        let summaries = transfer::summaries(&source);
        let (ua, ub, ta, tb) = (port(), port(), port(), port());
        let config = |udp, tcp, target| Config {
            udp,
            tcp,
            targets: vec![SocketAddr::from((Ipv4Addr::LOCALHOST, target))],
            test: true,
        };
        let (first, _) = start_config(a.path().into(), true, config(ua, ta, ub)).unwrap();
        let (second, events) = start_config(b.path().into(), true, config(ub, tb, ua)).unwrap();
        first.publish(summaries.clone());
        let end = Instant::now() + Duration::from_secs(20);
        let peer_id = loop {
            assert!(Instant::now() < end, "peer never discovered with entries");
            let event = events.recv_timeout(Duration::from_millis(500));
            if let Ok(SyncEvent::Peers(peers)) = event
                && let Some(peer) = peers.into_iter().find(|p| p.online && p.entries.len() == 3)
            {
                break peer.id;
            }
        };
        for entry in &summaries {
            let (received, paths) = second.restore(&peer_id, &entry.id).unwrap();
            assert_eq!(received.id, entry.id);
            assert_eq!(received.title, entry.title);
            if received.kind == Kind::Files {
                assert_eq!(fs::read(&paths[0]).unwrap(), [1, 3, 5]);
            }
            if received.kind == Kind::Image {
                assert_eq!(received.formats[0].data, png);
            }
        }
        let mut wrong = second
            .shared
            .peers
            .lock()
            .unwrap()
            .get(&peer_id)
            .unwrap()
            .clone();
        wrong.cert = identity(b.path()).unwrap().cert;
        assert!(connect(&wrong, &second.shared.enabled, Instant::now() + TIMEOUT).is_err());
        second.set_enabled(false).unwrap();
        assert!(second.restore(&peer_id, &summaries[0].id).is_err());
        let (reopened, _) =
            start_config(b.path().into(), true, config(port(), port(), port())).unwrap();
        assert!(!reopened.is_enabled());
        first.set_enabled(false).unwrap();
    }
    #[test]
    fn dribbling_tls_handshake_obeys_deadline() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let cert = identity(dir.path()).unwrap().cert;
        let (done_tx, done_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let _ = socket.write_all(&[0x16, 0x03, 0x03, 0x20, 0x00]);
            for _ in 0..30 {
                if socket.write_all(&[0]).is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(50));
            }
        });
        let client = thread::spawn(move || {
            let peer = Peer {
                name: "peer".into(),
                cert,
                addr,
                entries: vec![],
                seen: Instant::now(),
                online: true,
                polling: false,
                last_poll: None,
            };
            let enabled = AtomicBool::new(true);
            let result = connect(&peer, &enabled, Instant::now() + Duration::from_millis(300));
            let _ = done_tx.send(result.is_err());
        });
        let outcome = done_rx.recv_timeout(Duration::from_millis(600));
        server.join().unwrap();
        client.join().unwrap();
        assert_eq!(
            outcome,
            Ok(true),
            "TLS handshake must stop before the dribbling peer completes"
        );
    }
    #[test]
    fn disabling_interrupts_dribbling_tls_handshake() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let cert = identity(dir.path()).unwrap().cert;
        let enabled = Arc::new(AtomicBool::new(true));
        let (started_tx, started_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket.write_all(&[0x16, 0x03, 0x03, 0x20, 0x00]).unwrap();
            started_tx.send(()).unwrap();
            for _ in 0..40 {
                if socket.write_all(&[0]).is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(50));
            }
        });
        let flag = enabled.clone();
        let client = thread::spawn(move || {
            let peer = Peer {
                name: "peer".into(),
                cert,
                addr,
                entries: vec![],
                seen: Instant::now(),
                online: true,
                polling: false,
                last_poll: None,
            };
            let result = connect(&peer, &flag, Instant::now() + Duration::from_secs(5));
            let _ = done_tx.send(result.is_err());
        });
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        enabled.store(false, Ordering::SeqCst);
        let result = done_rx.recv_timeout(Duration::from_secs(1));
        server.join().unwrap();
        client.join().unwrap();
        assert_eq!(result, Ok(true));
    }
    #[test]
    fn disable_remains_effective_when_setting_cannot_be_persisted() {
        let dir = tempfile::tempdir().unwrap();
        let (handle, _) = start_config(
            dir.path().into(),
            false,
            Config {
                udp: port(),
                tcp: port(),
                targets: vec![],
                test: true,
            },
        )
        .unwrap();
        fs::create_dir(dir.path().join("sync-enabled")).unwrap();
        handle.shared.enabled.store(true, Ordering::SeqCst);
        assert!(handle.set_enabled(false).unwrap_err().contains("persist"));
        assert!(!handle.is_enabled());
    }
}
