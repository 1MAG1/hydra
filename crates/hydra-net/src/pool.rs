//! Idle-connection reuse for the ranged fetch path.
//!
//! # Why this matters more here than in a general HTTP client
//!
//! Every request used to carry `Connection: close`, so each one paid a fresh TCP
//! handshake and, on HTTPS, a fresh TLS handshake. For a client that fetches one
//! object per connection that is a fixed cost you notice once. For this one it is
//! the central cost: the whole premise is *n* concurrent ranges against the same
//! origin, so a transfer opened *n* connections to a host it was already talking
//! to, and every repair opened another.
//!
//! It also corrupts the scheduler's own arithmetic. The repair deadband and the
//! profitability test are both denominated in `delta`, the measured per-request
//! setup cost. With no reuse, `delta` is a full handshake — hundreds of
//! milliseconds through a proxy or on a distant TLS origin — so the scheduler
//! prices every decision against a number that connection reuse would cut by an
//! order of magnitude. Reuse does not merely make requests cheaper; it makes the
//! cost model describe something closer to the protocol's actual behaviour.
//!
//! # What may be reused, and what must never be
//!
//! A pooled connection is only safe when the client knows *exactly* where the
//! previous response ended, because anything left unread becomes the first bytes
//! of the next response. That is a silent corruption: the next range's body would
//! be prefixed with the previous one's tail, land at the wrong offsets, and still
//! produce a file of the right length.
//!
//! So a stream is returned to the pool only when all of these hold:
//!
//! * the response had a known length and the body was read to exactly that
//!   length — never a read-to-EOF body, and never a chunked body that did not
//!   reach its terminating zero-size chunk;
//! * the server did not answer `Connection: close`;
//! * the response was `HTTP/1.1` (a `1.0` peer without `keep-alive` closes);
//! * **the range was not shrunk mid-flight.**
//!
//! That last one is specific to this scheduler and easy to get wrong. When a
//! repair lowers a connection's far end, the fetch loop stops early *by design* —
//! the server is still sending toward the original end, so the socket has unread
//! body in it. Such a connection is exactly the "unknown amount left unread" case
//! and must be dropped, not pooled. Preemption stays free on the wire; what it
//! costs is the reusability of that one connection.

use crate::Target;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Identity of a pooled connection: the endpoint the socket is actually attached
/// to, plus whether TLS was negotiated over it.
///
/// `Target::host`/`port` are the CONNECT target — the proxy for a proxied fetch,
/// the origin otherwise — which is the correct key: two requests may share a
/// socket only if they would have dialled the same place. The TLS flag is part of
/// the key because a plaintext and a TLS stream to the same endpoint are not
/// interchangeable, and for a proxied TLS fetch the tunnel is already established
/// to a specific origin, so that origin is included too.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct PoolKey {
    host: String,
    port: u16,
    tls: bool,
    /// Set for a fetch through a forward proxy: a CONNECT tunnel is bound to one
    /// origin authority and cannot be handed to a request for a different one.
    via: Option<String>,
}

impl PoolKey {
    pub fn of(t: &Target) -> Self {
        PoolKey {
            host: t.host.clone(),
            port: t.port,
            tls: t.tls,
            via: t.origin.clone(),
        }
    }
}

/// How long an idle connection is offered for reuse.
///
/// Servers close idle keep-alive connections on their own schedule (nginx
/// defaults to 75 s, many CDNs to 5-10 s) and a stream closed by the peer looks
/// identical to a healthy one until the first read returns zero. Holding one for
/// less time than any common server timeout keeps that race rare; when it happens
/// anyway the caller retries on a fresh connection, so it is a slow path rather
/// than a failure.
const IDLE_TTL: Duration = Duration::from_secs(5);

/// Idle connections held per endpoint.
///
/// Bounded because the point is to reuse the connections a transfer already has,
/// not to accumulate sockets: with `n` connections working one origin, `n` idle
/// slots is the most that can ever be usefully waiting.
const MAX_IDLE_PER_KEY: usize = 8;

/// A set of idle streams available for reuse, keyed by endpoint.
///
/// Generic over the stream type so it works for the real TCP/TLS transport and
/// for the in-process duplex origin the hermetic tests use, exactly as
/// [`crate::Connector`] does.
pub struct ConnPool<S> {
    idle: Mutex<HashMap<PoolKey, Vec<(S, Instant)>>>,
    /// Reuses that were served from the pool, and connections opened fresh.
    /// Reported so a benchmark can state the reuse rate it actually achieved
    /// rather than assuming the pool worked.
    hits: std::sync::atomic::AtomicU64,
    misses: std::sync::atomic::AtomicU64,
}

impl<S> Default for ConnPool<S> {
    fn default() -> Self {
        Self::new()
    }
}

impl<S> ConnPool<S> {
    pub fn new() -> Self {
        ConnPool {
            idle: Mutex::new(HashMap::new()),
            hits: std::sync::atomic::AtomicU64::new(0),
            misses: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Take an idle stream for `t`, if one is available and not stale.
    ///
    /// Newest first: a connection idle for less time is likelier to still be open
    /// at the far end, and popping from the back is also the cheap end of the Vec.
    pub fn take(&self, t: &Target) -> Option<S> {
        let key = PoolKey::of(t);
        let now = Instant::now();
        let mut g = self.idle.lock().ok()?;
        let v = g.get_mut(&key)?;
        while let Some((s, since)) = v.pop() {
            if now.duration_since(since) < IDLE_TTL {
                self.hits.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                return Some(s);
            }
            // Stale: drop it and try the next one. Dropping closes the socket,
            // which is the correct disposition for a connection we no longer
            // believe in.
            drop(s);
        }
        None
    }

    /// Count a connection that had to be opened fresh.
    pub fn record_miss(&self) {
        self.misses
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    /// Offer a stream back for reuse.
    ///
    /// The caller is asserting that the previous response was consumed to its
    /// exact end. See the module docs for the full list of conditions; this
    /// function cannot verify them, which is why it is deliberately not called
    /// from any path that reads a body to EOF or that was shrunk mid-flight.
    pub fn put(&self, t: &Target, s: S) {
        let key = PoolKey::of(t);
        if let Ok(mut g) = self.idle.lock() {
            let v = g.entry(key).or_default();
            if v.len() < MAX_IDLE_PER_KEY {
                v.push((s, Instant::now()));
            }
            // Over the cap: drop it. An idle socket the transfer will not use is a
            // resource held on the server for nothing.
        }
    }

    /// `(reuses, fresh connections)` since this pool was created.
    pub fn stats(&self) -> (u64, u64) {
        (
            self.hits.load(std::sync::atomic::Ordering::Relaxed),
            self.misses.load(std::sync::atomic::Ordering::Relaxed),
        )
    }
}

/// Shared handle, cheap to clone into per-connection fetch tasks.
pub type SharedPool<S> = Arc<ConnPool<S>>;

#[cfg(test)]
mod tests {
    use super::*;

    fn t(host: &str, port: u16) -> Target {
        Target::direct(host, port, "/obj")
    }

    #[test]
    fn a_returned_connection_is_offered_back_for_the_same_endpoint() {
        let p: ConnPool<u32> = ConnPool::new();
        p.put(&t("a", 80), 7);
        assert_eq!(p.take(&t("a", 80)), Some(7));
        assert_eq!(p.take(&t("a", 80)), None, "taken twice");
    }

    /// The key must separate endpoints that are not interchangeable. Handing a
    /// socket connected to one host to a request for another would send the
    /// request to the wrong server — and with a plausible-looking response.
    #[test]
    fn connections_are_never_shared_across_endpoints() {
        let p: ConnPool<u32> = ConnPool::new();
        p.put(&t("a", 80), 1);
        assert_eq!(p.take(&t("b", 80)), None, "different host");
        assert_eq!(p.take(&t("a", 8080)), None, "different port");
        let mut tls = t("a", 80);
        tls.tls = true;
        assert_eq!(p.take(&tls), None, "plaintext stream offered to a TLS request");
        let mut proxied = t("a", 80);
        proxied.origin = Some("elsewhere:443".to_string());
        assert_eq!(
            p.take(&proxied),
            None,
            "a CONNECT tunnel is bound to one origin"
        );
        // The original request still gets it.
        assert_eq!(p.take(&t("a", 80)), Some(1));
    }

    #[test]
    fn idle_connections_are_capped_per_endpoint() {
        let p: ConnPool<u32> = ConnPool::new();
        for i in 0..(MAX_IDLE_PER_KEY as u32 + 5) {
            p.put(&t("a", 80), i);
        }
        let mut got = 0;
        while p.take(&t("a", 80)).is_some() {
            got += 1;
        }
        assert_eq!(got, MAX_IDLE_PER_KEY, "cap not enforced");
    }

    #[test]
    fn reuse_and_fresh_counts_are_reported() {
        let p: ConnPool<u32> = ConnPool::new();
        p.record_miss();
        p.put(&t("a", 80), 1);
        let _ = p.take(&t("a", 80));
        assert_eq!(p.stats(), (1, 1));
    }
}
