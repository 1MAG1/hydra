//! HTTP and HTTPS behind the [`Fetcher`] seam.
//!
//! A thin adapter over the existing transport, deliberately so: HTTP is the protocol the
//! scheduler was designed around, and rewriting it to fit a new abstraction would risk the
//! behaviour that 207 tests and a real-network experiment already cover. The adapter exists
//! so that HTTP and FTP are selected the same way and so that adding a third protocol
//! touches neither the scheduler nor the CLI.
//!
//! The one substantive thing it does is declare [`Capabilities::http`] — ranged reads,
//! client-bounded ranges, zero-round-trip preemption, real validators. Those four facts are
//! what the scheduling result depends on, and stating them next to FTP's makes the
//! difference between the protocols explicit rather than folkloric.

use crate::scheme::{Capabilities, Endpoint, Fetcher, SchemeProbe};
use crate::{Connector, SparseSink, Target};
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::Arc;

pub struct HttpFetcher<C: Connector> {
    conn: Arc<C>,
}

impl<C: Connector> HttpFetcher<C> {
    pub fn new(conn: Arc<C>) -> Self {
        Self { conn }
    }
}

/// Convert the protocol-neutral endpoint into the HTTP transport's target.
pub fn to_target(t: &Endpoint) -> Target {
    Target {
        host: t.host.clone(),
        port: t.port,
        path: t.path.clone(),
        origin: t.origin.as_ref().map(|(h, p)| format!("{h}:{p}")),
        tls: t.tls,
        headers: t.extra_headers.clone(),
        agent: t.agent.clone(),
    }
}

impl<C: Connector> Fetcher for HttpFetcher<C> {
    fn scheme(&self) -> &'static str {
        "http"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::http()
    }

    fn probe<'a>(
        &'a self,
        t: &'a Endpoint,
    ) -> Pin<Box<dyn Future<Output = io::Result<SchemeProbe>> + Send + 'a>> {
        Box::pin(async move {
            let target = to_target(t);
            let p = crate::probe(self.conn.as_ref(), &target).await?;
            Ok(SchemeProbe {
                size: p.size,
                ranged: p.ranges,
                validator: p.validator.clone(),
                weak_validator: p.weak_validator,
                content_type: p.content_type.clone(),
                raw: p.raw_head.clone(),
            })
        })
    }

    fn fetch_range<'a>(
        &'a self,
        t: &'a Endpoint,
        lo: u64,
        hi: u64,
        sink: Arc<SparseSink>,
    ) -> Pin<Box<dyn Future<Output = io::Result<()>> + Send + 'a>> {
        Box::pin(async move {
            let target = to_target(t);
            crate::fetch_range_retry(self.conn.clone(), target, lo, hi, sink, 3, 30.0).await
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_capabilities_state_the_property_the_scheduler_relies_on() {
        let f = HttpFetcher::new(Arc::new(crate::TcpConnector));
        let c = f.capabilities();
        assert!(
            c.client_bounded_ranges && c.preempt_cost_rtt == 0.0,
            "free preemption is the whole basis of the makespan result"
        );
        assert!(c.ranged && c.has_validators);
        assert_eq!(f.scheme(), "http");
    }

    #[test]
    fn tls_and_proxy_routing_survive_the_conversion() {
        // A dropped `tls` flag would silently downgrade an https:// fetch to plaintext, and
        // a dropped origin would send an absolute-form request to the origin server.
        let mut e = Endpoint::new("example.org", 443, "/f.bin");
        e.tls = true;
        e.origin = Some(("proxy.local".into(), 3128));
        let t = to_target(&e);
        assert!(t.tls, "an https endpoint must not be downgraded");
        assert_eq!(t.origin.as_deref(), Some("proxy.local:3128"));
        assert_eq!(t.port, 443);
    }
}
