// The dashboard's check that an address points somewhere, before a request that carries it is
// staged. It runs on the hosted service, so it must not reach inside the service's own network: the
// host is resolved once, every address must be public (platform/probe.rs), the connection is pinned
// to those addresses (no second lookup can swap in a private one), and redirects are not followed.
// A private or internal address is not refused: the machine that applies the request checks it from
// its own network before it keeps it.

use std::net::{SocketAddr, ToSocketAddrs};
use std::time::Duration;

use crate::platform::probe;

/** What the dashboard found about an address
 * Variants
    - Answered(String) - it answered as a service does
    - Private(String) - it is private or internal; the machine checks it
    - Failed(String) - it is malformed, does not resolve, or does not answer as a service
*/
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Found {
    Answered(String),
    Private(String),
    Failed(String),
}

/** Check an address from the hosted service
 * Input
    - url: &str - address
 * Output
    - Found
*/
pub(crate) fn check(url: &str) -> Found {
    let url = url.trim();
    if let Err(error) = probe::syntax(url) {
        return Found::Failed(error);
    }
    let Some((host, port)) = probe::host_and_port(url) else {
        return Found::Failed(format!("'{url}' has no host"));
    };
    if host == "localhost" || host.ends_with(".internal") || host.ends_with(".local") {
        return Found::Private(format!(
            "{host} is internal; the machine checks it when it applies the request"
        ));
    }
    let addresses: Vec<SocketAddr> = match (host.as_str(), port).to_socket_addrs() {
        Ok(found) => found.collect(),
        Err(error) => return Found::Failed(format!("{host} does not resolve: {error}")),
    };
    if addresses.is_empty() {
        return Found::Failed(format!("{host} does not resolve"));
    }
    if addresses
        .iter()
        .any(|address| !probe::is_public(address.ip()))
    {
        return Found::Private(format!(
            "{host} is a private address; the machine checks it from its own network when it applies the request"
        ));
    }
    let pinned = addresses.clone();
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(4))
        .timeout(Duration::from_secs(probe::PROBE_SECONDS))
        .redirects(0)
        .user_agent("crane-url-check")
        .resolver(move |_: &str| Ok(pinned.clone()))
        .build();
    let status = match agent.get(url).call() {
        Ok(response) => response.status(),
        Err(ureq::Error::Status(status, _)) => status,
        Err(error) => return Found::Failed(format!("{url} did not answer: {error}")),
    };
    match probe::verdict(url, status) {
        Ok(answer) => Found::Answered(answer),
        Err(error) => Found::Failed(error),
    }
}
