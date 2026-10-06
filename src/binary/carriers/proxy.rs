//! Forwards `/carriers/*` from the static server to the node sidecar.

use super::discovery::is_valid_name;
use crate::server::DynamicHandler;
use std::{
    io::Read,
    str::FromStr,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, RwLock,
    },
    thread,
    time::{Duration, Instant},
};
use tiny_http::{Header, Request, Response, StatusCode};
use tracing::{debug, warn};

/// A dev server is not a load balancer, so bodies are buffered rather than
/// streamed. This only bounds what one request may hand a carrier.
const MAX_BODY: usize = 100 * 1024 * 1024;
/// Enough that a page full of parallel carrier calls works, few enough that a
/// hung carrier cannot exhaust the machine's threads.
const MAX_CONCURRENT: usize = 32;
/// How long a request waits for a sidecar that is starting or restarting. A
/// restart is a process spawn plus an import, so this only expires when
/// something is actually wrong.
const STARTUP_GRACE: Duration = Duration::from_secs(30);
/// Sends per request. More than one only happens when a restart moved the
/// sidecar out from under it; the bound stops a restart loop from holding a
/// request open indefinitely.
const MAX_ATTEMPTS: u8 = 3;

/// Headers that describe one hop and must not be forwarded to the next.
const HOP_BY_HOP: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

#[derive(Debug, Clone)]
pub(crate) enum ProxyState {
    /// The site has no carriers, so every request is left to the static server.
    Idle,
    Starting,
    Ready {
        port: u16,
    },
    Failed(String),
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Route {
    Static,
    /// `upstream` is the path and query to forward on.
    Carrier {
        name: String,
        upstream: String,
    },
    /// Shaped like a carrier request but not one we will route.
    Reject,
}

/// Splits a raw request URL into a route. The query string is kept: the static
/// handler throws it away, but a carrier's first argument is built from it.
pub(crate) fn route(raw_url: &str) -> Route {
    let (path, query) = match raw_url.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (raw_url, None),
    };
    let rest = match path.strip_prefix("/carriers/") {
        Some(rest) => rest,
        None => return Route::Static,
    };
    let (name, tail) = match rest.split_once('/') {
        Some((name, tail)) => (name, Some(tail)),
        None => (rest, None),
    };
    if !is_valid_name(name) {
        return Route::Reject;
    }
    // Built by prefixing our own route, never by forwarding what the client
    // sent, so nothing can address the sidecar's control plane.
    let mut upstream = format!("/carrier/{}", name);
    if let Some(tail) = tail {
        upstream.push('/');
        upstream.push_str(tail);
    }
    if let Some(query) = query {
        upstream.push('?');
        upstream.push_str(query);
    }
    Route::Carrier {
        name: name.to_string(),
        upstream,
    }
}

pub(crate) struct CarrierProxy {
    state: Arc<RwLock<ProxyState>>,
    client: reqwest::blocking::Client,
    in_flight: Arc<AtomicUsize>,
}

impl CarrierProxy {
    pub fn new(state: Arc<RwLock<ProxyState>>) -> Self {
        Self {
            state,
            client: reqwest::blocking::Client::builder()
                // A carrier returning "redirect:/x" is a 302 the browser must
                // see. Following it here would swallow it.
                .redirect(reqwest::redirect::Policy::none())
                // Loopback traffic must not be handed to a configured proxy.
                .no_proxy()
                // A pooled connection the sidecar has already closed fails the
                // request that reuses it. Over loopback a fresh connection per
                // request costs nothing next to running the carrier.
                .pool_max_idle_per_host(0)
                .timeout(Duration::from_secs(120))
                .build()
                .expect("a blocking client with no TLS config always builds"),
            in_flight: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// The sidecar's port, waiting while it is being (re)started. A carrier
    /// edit restarts it, and a request that arrives mid-restart should be
    /// answered by the new one rather than fail against the old one's port.
    fn port(&self) -> Result<u16, Response<std::io::Cursor<Vec<u8>>>> {
        let deadline = Instant::now() + STARTUP_GRACE;
        loop {
            match &*self.state.read().unwrap() {
                ProxyState::Ready { port } => return Ok(*port),
                ProxyState::Idle => {
                    return Err(Response::from_string("Not Found").with_status_code(404))
                }
                ProxyState::Failed(message) => {
                    return Err(Response::from_string(message.to_owned()).with_status_code(502))
                }
                ProxyState::Starting if Instant::now() >= deadline => {
                    return Err(Response::from_string("Carriers are still starting")
                        .with_status_code(503)
                        .with_header(header("Retry-After: 1")))
                }
                ProxyState::Starting => {}
            }
            thread::sleep(Duration::from_millis(25));
        }
    }

    fn forward(&self, mut request: Request, upstream: String) {
        let port = match self.port() {
            Ok(port) => port,
            Err(response) => return respond(request, response),
        };

        let method = match reqwest::Method::from_str(request.method().as_str()) {
            Ok(method) => method,
            Err(_) => {
                return respond(
                    request,
                    Response::from_string("Unsupported method").with_status_code(405),
                )
            }
        };

        let mut headers = reqwest::header::HeaderMap::new();
        for header in request.headers() {
            let name = header.field.as_str().as_str().to_lowercase();
            if HOP_BY_HOP.contains(&name.as_str())
                || name == "host"
                || name == "content-length"
                // Never let a client's header reach the sidecar's control plane.
                || name == "x-archival-token"
            {
                continue;
            }
            if let (Ok(name), Ok(value)) = (
                reqwest::header::HeaderName::from_bytes(name.as_bytes()),
                reqwest::header::HeaderValue::from_str(header.value.as_str()),
            ) {
                headers.insert(name, value);
            }
        }

        let mut body = Vec::new();
        let limit = request.body_length().unwrap_or(0);
        if limit > MAX_BODY {
            return respond(
                request,
                Response::from_string("Request body is too large").with_status_code(413),
            );
        }
        if let Err(e) = request
            .as_reader()
            .take(MAX_BODY as u64 + 1)
            .read_to_end(&mut body)
        {
            return respond(
                request,
                Response::from_string(format!("Couldn't read the request body: {}", e))
                    .with_status_code(400),
            );
        }
        if body.len() > MAX_BODY {
            return respond(
                request,
                Response::from_string("Request body is too large").with_status_code(413),
            );
        }

        // A restart can begin between resolving the port and sending, so a
        // failure against a port that is no longer current is retried against
        // the replacement rather than reported. The sidecar that died never ran
        // the request.
        let mut port = port;
        let mut attempts_left = MAX_ATTEMPTS;
        let response = loop {
            let url = format!("http://127.0.0.1:{}{}", port, upstream);
            let attempt = self
                .client
                .request(method.clone(), &url)
                .headers(headers.clone())
                .body(body.clone())
                .send();
            match attempt {
                Ok(response) => break response,
                Err(e) => match self.port() {
                    Ok(current) if current != port && attempts_left > 1 => {
                        attempts_left -= 1;
                        debug!("carrier moved from {} to {}, retrying", port, current);
                        port = current;
                    }
                    _ => {
                        // Also logged: the body reaches whoever made the
                        // request, but the reason belongs in the dev server's
                        // own output too.
                        warn!("carrier request to {} failed: {}", url, e);
                        return respond(
                            request,
                            Response::from_string(format!(
                                "The carrier sidecar didn't answer: {}",
                                e
                            ))
                            .with_status_code(502),
                        );
                    }
                },
            }
        };

        let status = response.status().as_u16();
        let mut out_headers = vec![];
        for (name, value) in response.headers() {
            let name = name.as_str().to_lowercase();
            // tiny_http frames the response itself, and nothing here re-encodes
            // a body, so the upstream's framing headers would be lies.
            if HOP_BY_HOP.contains(&name.as_str())
                || name == "content-length"
                || name == "content-encoding"
            {
                continue;
            }
            if let (Ok(field), Ok(value)) = (
                tiny_http::HeaderField::from_str(&name),
                value.to_str().map(|v| v.to_string()),
            ) {
                if let Ok(value) = value.parse() {
                    out_headers.push(Header { field, value });
                }
            }
        }
        let bytes = match response.bytes() {
            Ok(bytes) => bytes.to_vec(),
            Err(e) => {
                return respond(
                    request,
                    Response::from_string(format!("The carrier's response failed: {}", e))
                        .with_status_code(502),
                )
            }
        };
        respond(
            request,
            Response::from_data(bytes)
                .with_status_code(StatusCode(status))
                .with_header_list(out_headers),
        );
    }
}

impl DynamicHandler for CarrierProxy {
    fn handle(&self, request: Request) -> Option<Request> {
        if matches!(*self.state.read().unwrap(), ProxyState::Idle) {
            return Some(request);
        }
        let upstream = match route(request.url()) {
            Route::Static => return Some(request),
            Route::Reject => {
                respond(
                    request,
                    Response::from_string("Not Found").with_status_code(404),
                );
                return None;
            }
            Route::Carrier { upstream, .. } => upstream,
        };
        // tiny_http's accept loop is serial: a request handled inline blocks
        // every other one, including a carrier's own fetch of this site.
        if self.in_flight.load(Ordering::SeqCst) >= MAX_CONCURRENT {
            respond(
                request,
                Response::from_string("Too many carrier requests in flight")
                    .with_status_code(503)
                    .with_header(header("Retry-After: 1")),
            );
            return None;
        }
        let state = self.state.clone();
        let client = self.client.clone();
        let in_flight = self.in_flight.clone();
        in_flight.fetch_add(1, Ordering::SeqCst);
        thread::spawn(move || {
            let proxy = CarrierProxy {
                state,
                client,
                in_flight: in_flight.clone(),
            };
            proxy.forward(request, upstream);
            in_flight.fetch_sub(1, Ordering::SeqCst);
        });
        None
    }
}

trait WithHeaderList {
    fn with_header_list(self, headers: Vec<Header>) -> Self;
}

impl<R: Read> WithHeaderList for Response<R> {
    fn with_header_list(mut self, headers: Vec<Header>) -> Self {
        for header in headers {
            self.add_header(header);
        }
        self
    }
}

fn header(raw: &str) -> Header {
    Header::from_str(raw).expect("formatted correctly")
}

fn respond<R: Read>(request: Request, response: Response<R>) {
    if let Err(e) = request.respond(response) {
        warn!("failed answering a carrier request: {}", e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn carrier(url: &str) -> (String, String) {
        match route(url) {
            Route::Carrier { name, upstream } => (name, upstream),
            other => panic!("expected a carrier route for {}, got {:?}", url, other),
        }
    }

    #[test]
    fn anything_outside_carriers_is_static() {
        assert_eq!(route("/"), Route::Static);
        assert_eq!(route("/index.html"), Route::Static);
        assert_eq!(route("/carriersfoo"), Route::Static);
        // The bare directory is not a carrier, and no name means nothing to route.
        assert_eq!(route("/carriers"), Route::Static);
    }

    #[test]
    fn the_query_string_is_forwarded() {
        // The static handler throws it away; a carrier's params come from it.
        assert_eq!(
            carrier("/carriers/echo?name=x&b=1"),
            ("echo".to_string(), "/carrier/echo?name=x&b=1".to_string())
        );
    }

    #[test]
    fn a_trailing_path_is_forwarded() {
        assert_eq!(
            carrier("/carriers/echo/a/b?q=1"),
            ("echo".to_string(), "/carrier/echo/a/b?q=1".to_string())
        );
    }

    #[test]
    fn a_bare_carrier_name_routes() {
        assert_eq!(
            carrier("/carriers/echo"),
            ("echo".to_string(), "/carrier/echo".to_string())
        );
    }

    #[test]
    fn nothing_can_address_the_control_plane() {
        assert_eq!(route("/carriers/../__control/state"), Route::Reject);
        assert_eq!(route("/carriers/..%2f__control/state"), Route::Reject);
        assert_eq!(route("/carriers/."), Route::Reject);
        assert_eq!(route("/carriers/"), Route::Reject);
        // Even a legal name only ever produces our own /carrier/ prefix.
        let (_, upstream) = carrier("/carriers/echo/__control/state");
        assert!(upstream.starts_with("/carrier/echo/"));
    }
}
