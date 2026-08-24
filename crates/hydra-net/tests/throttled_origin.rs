//! An origin that refuses concurrency must be met with less of it.
//!
//! A `429` is the server answering a question the client never asked: how many
//! requests at once will you accept? Standing down for `Retry-After` and coming
//! back with the same connection count asks it again and gets the same answer.
//! `ash-speed.hetzner.com` serves happily at two connections and refuses
//! everything at four, so eight connections downloaded zero bytes in thirty
//! seconds — one refusal every two seconds, each aborting what the other seven
//! had in flight. The transfer has to converge on a count the origin will serve.

use hya_core::{Scheduler, Source};
use hya_net::{run_transfer, Target, TlsCapableConnector};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const SIZE: u64 = 16 * 1024 * 1024;
/// What this origin will serve at once. Anything above it is refused.
const ALLOWED: usize = 2;

fn byte_at(off: u64) -> u8 {
    (off % 251) as u8
}

/// Serves ranges, but only [`ALLOWED`] at a time; the rest get `429`.
async fn spawn_throttled_origin(refusals: Arc<AtomicUsize>, peak: Arc<AtomicUsize>) -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = l.local_addr().expect("addr").port();
    let inflight = Arc::new(AtomicUsize::new(0));
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else {
                return;
            };
            let (inflight, refusals, peak) = (inflight.clone(), refusals.clone(), peak.clone());
            tokio::spawn(async move {
                let mut head = Vec::new();
                let mut buf = [0u8; 1024];
                while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                    match s.read(&mut buf).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => head.extend_from_slice(&buf[..n]),
                    }
                }
                let text = String::from_utf8_lossy(&head).to_string();
                let range = text
                    .lines()
                    .find(|l| l.to_ascii_lowercase().starts_with("range:"))
                    .and_then(|l| l.split_once(':').map(|(_, v)| v.trim().to_string()))
                    .unwrap_or_default();
                let spec = range.trim_start_matches("bytes=");
                let (lo, hi) = spec.split_once('-').unwrap_or(("0", ""));
                let lo: u64 = lo.parse().unwrap_or(0);
                let hi: u64 = hi.parse().unwrap_or(SIZE - 1);

                let n = inflight.fetch_add(1, Ordering::SeqCst) + 1;
                if n > ALLOWED {
                    inflight.fetch_sub(1, Ordering::SeqCst);
                    refusals.fetch_add(1, Ordering::SeqCst);
                    let _ = s
                        .write_all(
                            b"HTTP/1.1 429 Too Many Requests\r\nRetry-After: 1\r\n\
                              Content-Length: 0\r\nConnection: close\r\n\r\n",
                        )
                        .await;
                    return;
                }
                peak.fetch_max(n, Ordering::SeqCst);
                let len = hi - lo + 1;
                let head = format!(
                    "HTTP/1.1 206 Partial Content\r\nContent-Type: application/octet-stream\r\n\
                     Content-Length: {len}\r\nETag: \"throttled\"\r\n\
                     Content-Range: bytes {lo}-{hi}/{SIZE}\r\n\r\n"
                );
                let _ = s.write_all(head.as_bytes()).await;
                // In slices, with the request kept in flight long enough that
                // concurrent ones actually overlap.
                let mut off = lo;
                while off <= hi {
                    let end = (off + 32 * 1024 - 1).min(hi);
                    let body: Vec<u8> = (off..=end).map(byte_at).collect();
                    if s.write_all(&body).await.is_err() {
                        break;
                    }
                    off = end + 1;
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                inflight.fetch_sub(1, Ordering::SeqCst);
            });
        }
    });
    port
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_429_lowers_the_connection_count_instead_of_livelocking() {
    let refusals = Arc::new(AtomicUsize::new(0));
    // The most requests this origin serves at once ONCE THE CLIENT HAS SETTLED —
    // the counter is reset below, after the refusals have done their work.
    // Converging is only half the requirement: a client that answers a refusal by
    // collapsing to one connection has stopped being refused and is also
    // transferring at half the rate the origin was willing to give.
    let peak = Arc::new(AtomicUsize::new(0));
    let port = spawn_throttled_origin(refusals.clone(), peak.clone()).await;
    let conn = Arc::new(TlsCapableConnector::new().expect("client must build"));
    let t = Target::direct("127.0.0.1", port, "/obj");
    let out = std::env::temp_dir().join("hydra_throttled_origin.bin");
    let outs = out.to_string_lossy().to_string();

    // Eight connections against an origin that serves two: without a concurrency
    // response to the refusals, every round is refused exactly as the last one was.
    // Forget the opening burst: every client reaches the origin's limit before the
    // first refusal has been felt. What is being measured is what it does after.
    let settle = peak.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(1500)).await;
        settle.store(0, Ordering::SeqCst);
    });

    let t0 = std::time::Instant::now();
    let sched = Scheduler::new(SIZE, vec![Source::default()], &[8]).with_stall_timeout(3.0);
    let r = tokio::time::timeout(
        Duration::from_secs(60),
        run_transfer(conn, vec![t], &[8], SIZE, &outs, sched),
    )
    .await
    .expect("a throttled transfer must converge, not livelock");
    r.expect("the object must be delivered at a connection count the origin serves");

    let refused = refusals.load(Ordering::SeqCst);
    eprintln!(
        "refusals={refused} elapsed={:.2}s",
        t0.elapsed().as_secs_f64()
    );
    let data = std::fs::read(&out).expect("output");
    let _ = std::fs::remove_file(&out);
    assert_eq!(data.len() as u64, SIZE, "short file");
    let bad = data
        .iter()
        .enumerate()
        .find(|(i, b)| **b != byte_at(*i as u64));
    assert!(
        bad.is_none(),
        "content mismatch at byte {:?}",
        bad.map(|(i, _)| i)
    );
    assert!(
        refused > 0,
        "the origin must actually have refused, or this proves nothing"
    );
    // Converging costs a bounded number of refusals — one round per halving.
    // Retrying at the same count instead of lowering it measured 129 refusals
    // over 26 s on this origin against 8 over 2.2 s, so the ceiling separates
    // "learned the limit" from "kept asking".
    assert_eq!(
        peak.load(Ordering::SeqCst),
        ALLOWED,
        "the transfer must settle at the concurrency the origin serves, not below it"
    );
    assert!(
        refused <= 24,
        "{refused} refusals means the client never lowered its concurrency"
    );
}
