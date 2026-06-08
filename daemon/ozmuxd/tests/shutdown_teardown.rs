//! Verifies the daemon tears down a pre-`Attach` connection's reader/writer
//! threads on shutdown instead of leaking them (the conn-registry drain).

use ozmuxd::Server;
use std::io::Read;
use std::os::unix::net::UnixStream;
use std::time::Duration;

fn sock(name: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("ozmuxd-st-{name}.sock"));
    let _ = std::fs::remove_file(&p);
    p
}

#[test]
fn pre_attach_connection_is_torn_down_on_shutdown() {
    let path = sock("pre-attach-teardown");
    let handle = Server::new().serve(&path).unwrap();

    // Connect but NEVER send Hello: the reader thread blocks on read_message.
    let mut conn = UnixStream::connect(&path).unwrap();
    conn.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    std::thread::sleep(Duration::from_millis(50)); // let the reader thread spawn + block

    // Drop on a watchdog thread: a deadlocking drain_and_join (lock held across
    // join, vs the reader's self-remove which locks the same map) would hang here.
    let (tx, rx) = std::sync::mpsc::channel();
    let t = std::thread::spawn(move || {
        drop(handle);
        let _ = tx.send(());
    });
    assert!(
        rx.recv_timeout(Duration::from_secs(3)).is_ok(),
        "ServerHandle::Drop must not hang on a pre-Attach connection's blocked reader"
    );
    t.join().unwrap();

    // The drain shut the server side down, so the client observes EOF (read == 0),
    // proving the pre-Attach reader was unblocked + joined (not leaked, still holding the fd).
    let mut buf = [0u8; 1];
    let n = conn.read(&mut buf).expect(
        "read after shutdown must return EOF, not time out (a leaked reader holds the fd open)",
    );
    assert_eq!(
        n, 0,
        "pre-Attach connection must be shut down (EOF) after ServerHandle drop"
    );
}
