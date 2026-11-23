use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use super::wire::{Decoder, Encoder};
use super::*;

// --- Helpers -----------------------------------------------------------------

/// A service name of the test's own, so tests running in parallel — and two
/// runs of the suite at once — never meet.
fn service(test: &str) -> String {
    static SEQ: AtomicUsize = AtomicUsize::new(0);
    format!(
        "test.{test}.{}.{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    )
}

fn encode(values: Vec<Wire>) -> (Vec<u8>, Vec<Handle>) {
    let mut export = |key: u64, _: &str| key as u32 + 100;
    let mut enc = Encoder::new(&mut export);
    enc.values(values);
    (enc.bytes, enc.fds)
}

fn decode(bytes: &[u8], fds: Vec<Handle>) -> Result<Vec<Wire>, wire::DecodeError> {
    let mut lookup = |id: u32| (id >= 100).then(|| id as u64 - 100);
    let mut imported = |_: u32| {};
    let mut d = Decoder::new(bytes, fds, &mut lookup, &mut imported);
    let values = d.values()?;
    d.finish()?;
    Ok(values)
}

/// Run a server on its own thread: publish its root as `name`, serve with
/// `handler` until `until` says so or 10 seconds pass.
fn serve<H: Handler + Send + 'static>(
    name: String,
    mut handler: H,
    until: impl Fn(&H) -> bool + Send + 'static,
) -> thread::JoinHandle<H> {
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let t = thread::spawn(move || {
        let mut looper = Looper::new();
        looper.publish(&name, 0).unwrap();
        ready_tx.send(()).unwrap();
        let start = Instant::now();
        while !until(&handler) && start.elapsed() < Duration::from_secs(10) {
            looper
                .dispatch(&mut handler, Some(Duration::from_millis(20)))
                .unwrap();
        }
        handler
    });
    ready_rx.recv().unwrap();
    t
}

/// A handler for the client side, which mostly only calls.
#[derive(Default)]
struct Client {
    calls: Vec<(u64, String)>,
    disconnected: bool,
}

impl Handler for Client {
    fn call(
        &mut self,
        _: &mut Looper,
        _: ConnId,
        key: u64,
        method: &str,
        args: Vec<Wire>,
    ) -> Result<Vec<Wire>, String> {
        self.calls.push((key, method.to_owned()));
        match (method, args.as_slice()) {
            ("double", [Wire::Int(n)]) => Ok(vec![Wire::Int(n * 2)]),
            _ => Ok(vec![]),
        }
    }

    fn disconnected(&mut self, _: &mut Looper, _: ConnId) {
        self.disconnected = true;
    }
}

// --- Wire --------------------------------------------------------------------

#[test]
fn every_kind_of_value_survives_the_round_trip() {
    let shm = SharedMemory::create(4096).unwrap();
    let values = vec![
        Wire::Nil,
        Wire::Bool(true),
        Wire::Bool(false),
        Wire::Int(-42),
        Wire::Int(i64::MAX),
        Wire::Float(1.5),
        Wire::Str("héllo".into()),
        Wire::Bytes(vec![0, 255, 7]),
        Wire::List(vec![Wire::Int(1), Wire::Str("two".into())]),
        Wire::Map(vec![(Wire::Str("k".into()), Wire::Bool(true))]),
        Wire::Record {
            class: "KeyEvent".into(),
            fields: vec![("key".into(), Wire::Str("a".into())), ("ctrl".into(), Wire::Bool(false))],
        },
        // Sent as ours, arrives as theirs — and back.
        Wire::Object(ObjRef::Local {
            key: 7,
            interface: "WindowEvents".into(),
        }),
        Wire::Object(ObjRef::Remote {
            id: 103,
            interface: "Window".into(),
        }),
        Wire::Shm(shm.share().unwrap(), 4096),
    ];
    let (bytes, fds) = encode(values);
    assert_eq!(fds.len(), 1);
    let back = decode(&bytes, fds).unwrap();
    let text = format!("{back:?}");
    for expected in [
        "Nil",
        "Int(-42)",
        "Int(9223372036854775807)",
        "Float(1.5)",
        "Str(\"héllo\")",
        "Bytes([0, 255, 7])",
        "class: \"KeyEvent\"",
        // Our local key 7 was exported as id 107: the receiver sees one of
        // the sender's objects.
        "Remote { id: 107, interface: \"WindowEvents\" }",
        // Its id 103 came back to it: its own object, key 3.
        "Local { key: 3, interface: \"Window\" }",
    ] {
        assert!(text.contains(expected), "missing {expected} in {text}");
    }
    assert!(matches!(back.last(), Some(Wire::Shm(_, 4096))));
}

#[test]
fn malformed_messages_are_rejected_not_trusted() {
    // Truncated.
    let (bytes, _) = encode(vec![Wire::Str("hello".into())]);
    assert!(decode(&bytes[..bytes.len() - 1], vec![]).is_err());
    // An unknown tag.
    assert!(decode(&[1, 0, 0, 0, 99], vec![]).is_err());
    // A list claiming four billion items in a five-byte message.
    assert!(decode(&[1, 0, 0, 0, 7, 255, 255, 255, 255], vec![]).is_err());
    // A descriptor the message does not carry.
    let (bytes, _) = encode(vec![Wire::Shm(SharedMemory::create(8).unwrap().share().unwrap(), 8)]);
    assert!(decode(&bytes, vec![]).is_err());
    // A reference to an object never exported.
    let mut lookup = |_: u32| None;
    let mut imported = |_: u32| {};
    let (bytes, _) = encode(vec![Wire::Object(ObjRef::Remote {
        id: 5,
        interface: "X".into(),
    })]);
    let mut d = Decoder::new(&bytes, vec![], &mut lookup, &mut imported);
    assert!(d.values().is_err());
    // Nesting past the limit.
    let mut deep = Wire::Nil;
    for _ in 0..(wire::MAX_DEPTH + 2) {
        deep = Wire::List(vec![deep]);
    }
    let (bytes, _) = encode(vec![deep]);
    assert!(decode(&bytes, vec![]).is_err());
    // Bytes after the end.
    let (mut bytes, _) = encode(vec![Wire::Nil]);
    bytes.push(0);
    assert!(decode(&bytes, vec![]).is_err());
}

// --- Shared memory -----------------------------------------------------------

#[test]
fn shared_memory_is_the_same_bytes_through_another_descriptor() {
    let mut a = SharedMemory::create(64).unwrap();
    a.as_mut_slice()[..5].copy_from_slice(b"hello");
    let mut b = SharedMemory::from_fd(a.share().unwrap(), 64).unwrap();
    assert_eq!(&b.as_slice()[..5], b"hello");
    b.as_mut_slice()[0] = b'J';
    assert_eq!(&a.as_slice()[..5], b"Jello");
}

#[test]
fn mapping_more_than_was_shared_is_refused() {
    let a = SharedMemory::create(64).unwrap();
    assert!(SharedMemory::from_fd(a.share().unwrap(), 1 << 20).is_err());
}

// --- Services ------------------------------------------------------------------

#[test]
fn service_names_are_checked() {
    for good in ["saule.display", "a", "net.wifi-2", "x_y"] {
        assert!(valid_name(good), "{good}");
    }
    for bad in ["", "Saule", "1abc", "../etc", "a/b", "a b", &"a".repeat(65)] {
        assert!(!valid_name(bad), "{bad}");
    }
}

#[test]
fn a_name_already_served_cannot_be_published_twice() {
    let name = service("twice");
    let mut first = Looper::new();
    first.publish(&name, 0).unwrap();
    let mut second = Looper::new();
    let err = second.publish(&name, 0).unwrap_err();
    assert!(err.to_string().contains("already published"), "{err}");
    // Once the first lets go, the name is free again.
    drop(first);
    second.publish(&name, 0).unwrap();
}

#[cfg(unix)]
#[test]
fn a_stale_socket_file_is_replaced() {
    // A process that died leaves its socket file behind; nobody answers on
    // it, so publishing again takes it over.
    let dir = std::env::temp_dir().join(format!("sipc-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("stale");
    let _ = std::fs::remove_file(&path);
    drop(std::os::unix::net::UnixListener::bind(&path).unwrap());
    let mut looper = Looper::new();
    looper.publish_at(&path, "test.stale", 0).unwrap();
    assert!(looper.connect_at(&path).is_ok());
}

#[test]
fn connecting_to_a_service_nobody_publishes_says_so() {
    let mut looper = Looper::new();
    let err = looper.connect(&service("nobody")).unwrap_err();
    assert!(err.to_string().contains("is published"), "{err}");
    let err = looper.connect("Not A Name").unwrap_err();
    assert!(err.to_string().contains("is not a service name"), "{err}");
}

// --- Calls ---------------------------------------------------------------------

/// A server whose root object answers a few methods.
#[derive(Default)]
struct Server {
    one_way: usize,
    released: Vec<u64>,
    callbacks: Vec<i64>,
    connected: usize,
    disconnected: usize,
    stop: bool,
}

impl Handler for Server {
    fn call(
        &mut self,
        looper: &mut Looper,
        from: ConnId,
        key: u64,
        method: &str,
        args: Vec<Wire>,
    ) -> Result<Vec<Wire>, String> {
        match (key, method, args.as_slice()) {
            (0, "add", [Wire::Int(a), Wire::Int(b)]) => Ok(vec![Wire::Int(a + b)]),
            (0, "ping", []) => {
                self.one_way += 1;
                Ok(vec![])
            }
            (0, "fail", []) => Err("it went wrong on purpose".into()),
            // Hand out a new object, which the client will release.
            (0, "newObject", []) => Ok(vec![Wire::Object(ObjRef::Local {
                key: 42,
                interface: "Thing".into(),
            })]),
            (42, "name", []) => Ok(vec![Wire::Str("thing 42".into())]),
            // Call the client back *before* answering: the client is waiting
            // for this reply and must serve the callback meanwhile.
            (0, "callMeBack", [Wire::Object(ObjRef::Remote { id, .. })]) => {
                let id = *id;
                let r = looper
                    .call(self, from, id, "double", vec![Wire::Int(21)])
                    .map_err(|e| e.to_string())?;
                if let [Wire::Int(n)] = r.as_slice() {
                    self.callbacks.push(*n);
                }
                looper.release(from, id);
                Ok(vec![Wire::Str("called back".into())])
            }
            (0, "readShared", [Wire::Shm(..)]) => {
                let Some(Wire::Shm(fd, len)) = args.into_iter().next() else {
                    unreachable!()
                };
                let mut mem = SharedMemory::from_fd(fd, len as usize).map_err(|e| e.to_string())?;
                let text = String::from_utf8_lossy(&mem.as_slice()[..5]).into_owned();
                mem.as_mut_slice()[..5].copy_from_slice(b"WORLD");
                Ok(vec![Wire::Str(text)])
            }
            (0, "whoAmI", []) => {
                let peer = looper.peer(from).ok_or("no peer")?;
                Ok(vec![
                    Wire::Int(peer.pid.map_or(-1, |p| p as i64)),
                    Wire::Int(peer.uid as i64),
                ])
            }
            (0, "hangUp", []) => {
                looper.close(from);
                Ok(vec![])
            }
            (0, "stop", []) => {
                self.stop = true;
                Ok(vec![])
            }
            _ => Err(format!("no method `{method}` on object {key}")),
        }
    }

    fn connected(&mut self, _: &mut Looper, _: ConnId, service: &str) {
        assert!(service.starts_with("test."), "{service}");
        self.connected += 1;
    }

    fn disconnected(&mut self, _: &mut Looper, _: ConnId) {
        self.disconnected += 1;
    }

    fn released(&mut self, _: &mut Looper, _: ConnId, key: u64) {
        self.released.push(key);
    }
}

#[test]
fn a_call_returns_its_result_and_a_fault_returns_its_message() {
    let name = service("calls");
    let server = serve(name.clone(), Server::default(), |s| s.stop);
    let mut looper = Looper::new();
    let mut client = Client::default();
    let conn = looper.connect(&name).unwrap();

    let r = looper
        .call(&mut client, conn, ROOT, "add", vec![Wire::Int(2), Wire::Int(40)])
        .unwrap();
    assert!(matches!(r.as_slice(), [Wire::Int(42)]));

    let err = looper.call(&mut client, conn, ROOT, "fail", vec![]).unwrap_err();
    assert_eq!(err, CallError::Remote("it went wrong on purpose".into()));

    let err = looper.call(&mut client, conn, ROOT, "nope", vec![]).unwrap_err();
    assert!(matches!(err, CallError::Remote(m) if m.contains("no method `nope`")));

    looper.call(&mut client, conn, ROOT, "stop", vec![]).unwrap();
    let s = server.join().unwrap();
    assert_eq!(s.connected, 1);
}

#[test]
fn one_way_calls_arrive_in_order_without_replies() {
    let name = service("oneway");
    let server = serve(name.clone(), Server::default(), |s| s.stop);
    let mut looper = Looper::new();
    let mut client = Client::default();
    let conn = looper.connect(&name).unwrap();
    for _ in 0..100 {
        looper.send(conn, ROOT, "ping", vec![]).unwrap();
    }
    // A call with a reply after them: by the time it is answered, all 100
    // were handled, because one connection's messages run in order.
    looper.call(&mut client, conn, ROOT, "stop", vec![]).unwrap();
    assert_eq!(server.join().unwrap().one_way, 100);
}

#[test]
fn a_waiting_caller_serves_the_callback_its_call_makes() {
    let name = service("callback");
    let server = serve(name.clone(), Server::default(), |s| s.stop);
    let mut looper = Looper::new();
    let mut client = Client::default();
    let conn = looper.connect(&name).unwrap();

    let me = Wire::Object(ObjRef::Local {
        key: 9,
        interface: "Doubler".into(),
    });
    let r = looper
        .call(&mut client, conn, ROOT, "callMeBack", vec![me])
        .unwrap();
    assert!(matches!(r.as_slice(), [Wire::Str(s)] if s == "called back"));
    // The server called our object 9 while we were waiting.
    assert_eq!(client.calls, [(9, "double".to_owned())]);

    looper.call(&mut client, conn, ROOT, "stop", vec![]).unwrap();
    assert_eq!(server.join().unwrap().callbacks, [42]);
}

#[test]
fn calls_can_be_started_together_and_collected_later() {
    let name = service("async");
    let server = serve(name.clone(), Server::default(), |s| s.stop);
    let mut looper = Looper::new();
    let mut client = Client::default();
    let conn = looper.connect(&name).unwrap();

    let calls: Vec<CallId> = (0..10)
        .map(|i| {
            looper
                .call_async(conn, ROOT, "add", vec![Wire::Int(i), Wire::Int(100)])
                .unwrap()
        })
        .collect();
    // Collected in reverse: each waits for its own reply, whatever order.
    for (i, call) in calls.into_iter().enumerate().rev() {
        let r = looper.wait(&mut client, call).unwrap();
        assert!(matches!(r.as_slice(), [Wire::Int(n)] if *n == i as i64 + 100));
    }
    looper.call(&mut client, conn, ROOT, "stop", vec![]).unwrap();
    server.join().unwrap();
}

#[test]
fn shared_memory_crosses_and_both_sides_see_each_others_writes() {
    let name = service("shm");
    let server = serve(name.clone(), Server::default(), |s| s.stop);
    let mut looper = Looper::new();
    let mut client = Client::default();
    let conn = looper.connect(&name).unwrap();

    let mut mem = SharedMemory::create(1024 * 600 * 4).unwrap();
    mem.as_mut_slice()[..5].copy_from_slice(b"hello");
    let r = looper
        .call(
            &mut client,
            conn,
            ROOT,
            "readShared",
            vec![Wire::Shm(mem.share().unwrap(), mem.len() as u64)],
        )
        .unwrap();
    assert!(matches!(r.as_slice(), [Wire::Str(s)] if s == "hello"));
    // The server wrote into the same memory.
    assert_eq!(&mem.as_slice()[..5], b"WORLD");

    looper.call(&mut client, conn, ROOT, "stop", vec![]).unwrap();
    server.join().unwrap();
}

#[test]
fn an_object_handed_out_is_released_when_the_holder_lets_go() {
    let name = service("release");
    let server = serve(name.clone(), Server::default(), |s| s.stop);
    let mut looper = Looper::new();
    let mut client = Client::default();
    let conn = looper.connect(&name).unwrap();

    // Handed out twice: two references, so the release must carry both.
    let mut id = 0;
    for _ in 0..2 {
        let r = looper.call(&mut client, conn, ROOT, "newObject", vec![]).unwrap();
        let [Wire::Object(ObjRef::Remote { id: got, .. })] = r.as_slice() else {
            panic!("expected an object, got {r:?}");
        };
        id = *got;
    }
    let name = looper.call(&mut client, conn, id, "name", vec![]).unwrap();
    assert!(matches!(name.as_slice(), [Wire::Str(s)] if s == "thing 42"));

    looper.release(conn, id);
    looper.call(&mut client, conn, ROOT, "stop", vec![]).unwrap();
    let s = server.join().unwrap();
    assert_eq!(s.released, [42], "released once both references came back");
}

#[test]
fn a_connection_closing_fails_the_call_waiting_on_it() {
    let name = service("hangup");
    let server = serve(name.clone(), Server::default(), |s| s.disconnected > 0);
    let mut looper = Looper::new();
    let mut client = Client::default();
    let conn = looper.connect(&name).unwrap();

    // The server closes the connection while handling this call, so the
    // reply never comes.
    let err = looper.call(&mut client, conn, ROOT, "hangUp", vec![]);
    assert_eq!(err.unwrap_err(), CallError::Disconnected);
    assert!(client.disconnected);
    assert!(!looper.is_open(conn));
    // Calls on it now fail straight away.
    assert_eq!(
        looper.send(conn, ROOT, "ping", vec![]).unwrap_err(),
        CallError::Disconnected
    );
    assert_eq!(server.join().unwrap().disconnected, 1);
}

#[test]
fn the_server_learns_who_is_calling_from_the_kernel() {
    let name = service("peer");
    let server = serve(name.clone(), Server::default(), |s| s.stop);
    let mut looper = Looper::new();
    let mut client = Client::default();
    let conn = looper.connect(&name).unwrap();
    let r = looper.call(&mut client, conn, ROOT, "whoAmI", vec![]).unwrap();
    // Same process here (two threads), so the peer is us.
    let [Wire::Int(pid), Wire::Int(_uid)] = r.as_slice() else {
        panic!("{r:?}");
    };
    assert_eq!(*pid, std::process::id() as i64);
    #[cfg(unix)]
    {
        // Safety: no arguments.
        let uid = unsafe { libc::getuid() } as i64;
        assert_eq!(*_uid, uid);
        assert!(looper.socket(conn).is_some());
    }
    looper.call(&mut client, conn, ROOT, "stop", vec![]).unwrap();
    server.join().unwrap();
}

#[test]
fn a_call_too_large_to_send_is_refused_without_closing_the_connection() {
    let name = service("big");
    let server = serve(name.clone(), Server::default(), |s| s.stop);
    let mut looper = Looper::new();
    let mut client = Client::default();
    let conn = looper.connect(&name).unwrap();
    let huge = Wire::Bytes(vec![0; 17 << 20]);
    let err = looper.send(conn, ROOT, "ping", vec![huge]).unwrap_err();
    assert!(matches!(err, CallError::Invalid(_)), "{err:?}");
    // Still usable.
    let r = looper
        .call(&mut client, conn, ROOT, "add", vec![Wire::Int(1), Wire::Int(1)])
        .unwrap();
    assert!(matches!(r.as_slice(), [Wire::Int(2)]));
    looper.call(&mut client, conn, ROOT, "stop", vec![]).unwrap();
    server.join().unwrap();
}

#[test]
fn a_peer_that_stops_reading_is_cut_off_instead_of_blocking_the_sender() {
    // The display server's guarantee: sending to a frozen app never blocks,
    // and its backlog cannot grow without bound. The "app" here is a
    // service whose looper never runs, so nothing ever reads.
    let name = service("frozen");
    let mut frozen = Looper::new();
    frozen.publish(&name, 0).unwrap();
    let mut looper = Looper::new();
    let conn = looper.connect(&name).unwrap();
    let chunk = vec![0u8; 1 << 20];
    let start = Instant::now();
    let mut sent = 0;
    let err = loop {
        match looper.send(conn, ROOT, "frame", vec![Wire::Bytes(chunk.clone())]) {
            Ok(()) => sent += 1,
            Err(e) => break e,
        }
        assert!(sent < 1000, "never cut off");
    };
    assert_eq!(err, CallError::Disconnected);
    assert!(start.elapsed() < Duration::from_secs(5), "sending blocked");
    // Cut off after roughly the backlog limit, not before.
    assert!(sent >= 60, "cut off after only {sent} MiB");
    assert!(!looper.is_open(conn));
}
