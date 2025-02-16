mod request;
mod response;

use clap::Parser;
use rand::{Rng, SeedableRng};
use std::{collections::HashMap, sync::Arc};
use tokio::{net::{TcpListener, TcpStream}, sync::RwLock, time};

/// Contains information parsed from the command-line invocation of balancebeam. The Clap macros
/// provide a fancy way to automatically construct a command-line argument parser.
#[derive(Parser, Debug)]
#[command(about = "Fun with load balancing")]
struct CmdOptions {
    /// "IP/port to bind to"
    #[arg(short, long, default_value = "0.0.0.0:1100")]
    bind: String,
    /// "Upstream host to forward requests to"
    #[arg(short, long)]
    upstream: Vec<String>,
    /// "Perform active health checks on this interval (in seconds)"
    #[arg(long, default_value = "10")]
    active_health_check_interval: usize,
    /// "Path to send request to for active health checks"
    #[arg(long, default_value = "/")]
    active_health_check_path: String,
    /// "Maximum number of requests to accept per IP per minute (0 = unlimited)"
    #[arg(long, default_value = "0")]
    max_requests_per_minute: usize,
}

/// Contains information about the state of balancebeam (e.g. what servers we are currently proxying
/// to, what servers have failed, rate limiting counts, etc.)
///
/// You should add fields to this struct in later milestones.
struct ProxyState {
    /// How frequently we check whether upstream servers are alive (Milestone 4)
    #[allow(dead_code)]
    active_health_check_interval: usize,
    /// Where we should send requests when doing active health checks (Milestone 4)
    #[allow(dead_code)]
    active_health_check_path: String,
    /// Maximum number of requests an individual IP can make in a minute (Milestone 5)
    #[allow(dead_code)]
    max_requests_per_minute: usize,
    /// Addresses of servers that we are proxying to
    upstream_addresses: Vec<String>,
    /// Flags that indicate whether the upstream server is alive
    upstream_address_flags: Vec<bool>,
    /// Number of alive upstream servers
    upstream_address_alive_num: usize,
    /// Counter for each IP
    rate_limiting_counter: HashMap<String, usize>,
}

#[tokio::main]
async fn main() {
    // Initialize the logging library. You can print log messages using the `log` macros:
    // https://docs.rs/log/0.4.8/log/ You are welcome to continue using print! statements; this
    // just looks a little prettier.
    if let Err(_) = std::env::var("RUST_LOG") {
        std::env::set_var("RUST_LOG", "debug");
    }
    pretty_env_logger::init();

    // Parse the command line arguments passed to this program
    let options = CmdOptions::parse();
    if options.upstream.len() < 1 {
        log::error!("At least one upstream server must be specified using the --upstream option.");
        std::process::exit(1);
    }

    // Start listening for connections
    let listener = match TcpListener::bind(&options.bind).await {
        Ok(listener) => listener,
        Err(err) => {
            log::error!("Could not bind to {}: {}", options.bind, err);
            std::process::exit(1);
        }
    };
    log::info!("Listening for requests on {}", options.bind);

    // Handle incoming connections
    let upstream_address_num = options.upstream.len();
    let state = Arc::new(RwLock::new(ProxyState {
        upstream_addresses: options.upstream,
        active_health_check_interval: options.active_health_check_interval,
        active_health_check_path: options.active_health_check_path,
        max_requests_per_minute: options.max_requests_per_minute,
        upstream_address_flags: vec![true; upstream_address_num],
        upstream_address_alive_num: upstream_address_num,
        rate_limiting_counter: HashMap::new(),
    }));

    let state_ref = state.clone();
    tokio::spawn(async move {
        active_health_check(&state_ref).await;
    });

    let state_ref = state.clone();
    tokio::spawn(async move {
        rate_limiting_counter_clear(&state_ref).await;
    });

    loop {
        if let Ok((stream, _)) = listener.accept().await {
            let state_ref = state.clone();
            tokio::spawn(async move {
                handle_connection(stream, &state_ref).await;
            });
        }
    }
}

async fn connect_to_upstream(state: &RwLock<ProxyState>) -> Result<TcpStream, std::io::Error> {
    let mut rng = rand::rngs::StdRng::from_entropy();
    // let upstream_idx = rng.gen_range(0..state.upstream_addresses.len());
    // let upstream_ip = &state.upstream_addresses[upstream_idx];
    // TcpStream::connect(upstream_ip).await.or_else(|err| {
    //     log::error!("Failed to connect to upstream {}: {}", upstream_ip, err);
    //     Err(err)
    // })
    // TODO: implement failover (milestone 3)
    loop {
        let state_r = state.read().await;
        if state_r.upstream_address_alive_num == 0 {
            return Err(std::io::Error::new(std::io::ErrorKind::Other, "No alive upstream addresses"));
        }
        let upstream_idx = rng.gen_range(0..state_r.upstream_addresses.len());
        if !state_r.upstream_address_flags[upstream_idx] {
            continue;
        }
        let upstream_ip = &state_r.upstream_addresses[upstream_idx];
        match TcpStream::connect(upstream_ip).await {
            Ok(stream) => {
                return Ok(stream);
            }
            Err(err) => {
                log::error!("Failed to connect to upstream {}: {}", upstream_ip, err);
                drop(state_r);
                let mut state_w = state.write().await;
                state_w.upstream_address_flags[upstream_idx] = false;
                state_w.upstream_address_alive_num -= 1;
            }
        }
    }
}

async fn send_response(client_conn: &mut TcpStream, response: &http::Response<Vec<u8>>) {
    let client_ip = client_conn.peer_addr().unwrap().ip().to_string();
    log::info!("{} <- {}", client_ip, response::format_response_line(&response));
    if let Err(error) = response::write_to_stream(&response, client_conn).await {
        log::warn!("Failed to send response to client: {}", error);
        return;
    }
}

async fn handle_connection(mut client_conn: TcpStream, state: &RwLock<ProxyState>) {
    let client_ip = client_conn.peer_addr().unwrap().ip().to_string();
    log::info!("Connection received from {}", client_ip);

    // Open a connection to a random destination server
    let mut upstream_conn = match connect_to_upstream(state).await {
        Ok(stream) => stream,
        Err(_error) => {
            let response = response::make_http_error(http::StatusCode::BAD_GATEWAY);
            send_response(&mut client_conn, &response).await;
            return;
        }
    };
    let upstream_ip = upstream_conn.peer_addr().unwrap().ip().to_string();

    // The client may now send us one or more requests. Keep trying to read requests until the
    // client hangs up or we get an error.
    loop {
        // Read a request from the client
        let mut request = match request::read_from_stream(&mut client_conn).await {
            Ok(request) => request,
            // Handle case where client closed connection and is no longer sending requests
            Err(request::Error::IncompleteRequest(0)) => {
                log::debug!("Client finished sending requests. Shutting down connection");
                return;
            }
            // Handle I/O error in reading from the client
            Err(request::Error::ConnectionError(io_err)) => {
                log::info!("Error reading request from client stream: {}", io_err);
                return;
            }
            Err(error) => {
                log::debug!("Error parsing request: {:?}", error);
                let response = response::make_http_error(match error {
                    request::Error::IncompleteRequest(_)
                    | request::Error::MalformedRequest(_)
                    | request::Error::InvalidContentLength
                    | request::Error::ContentLengthMismatch => http::StatusCode::BAD_REQUEST,
                    request::Error::RequestBodyTooLarge => http::StatusCode::PAYLOAD_TOO_LARGE,
                    request::Error::ConnectionError(_) => http::StatusCode::SERVICE_UNAVAILABLE,
                });
                send_response(&mut client_conn, &response).await;
                continue;
            }
        };
        log::info!(
            "{} -> {}: {}",
            client_ip,
            upstream_ip,
            request::format_request_line(&request)
        );

        if let Err(_) = rate_limiting_check(state, &client_ip).await {
            let response = response::make_http_error(http::StatusCode::TOO_MANY_REQUESTS);
            send_response(&mut client_conn, &response).await;
            continue;
        }

        // Add X-Forwarded-For header so that the upstream server knows the client's IP address.
        // (We're the ones connecting directly to the upstream server, so without this header, the
        // upstream server will only know our IP, not the client's.)
        request::extend_header_value(&mut request, "x-forwarded-for", &client_ip);

        // Forward the request to the server
        if let Err(error) = request::write_to_stream(&request, &mut upstream_conn).await {
            log::error!("Failed to send request to upstream {}: {}", upstream_ip, error);
            let response = response::make_http_error(http::StatusCode::BAD_GATEWAY);
            send_response(&mut client_conn, &response).await;
            return;
        }
        log::debug!("Forwarded request to server");

        // Read the server's response
        let response = match response::read_from_stream(&mut upstream_conn, request.method()).await {
            Ok(response) => response,
            Err(error) => {
                log::error!("Error reading response from server: {:?}", error);
                let response = response::make_http_error(http::StatusCode::BAD_GATEWAY);
                send_response(&mut client_conn, &response).await;
                return;
            }
        };
        // Forward the response to the client
        send_response(&mut client_conn, &response).await;
        log::debug!("Forwarded response to client");
    }
}

async fn active_health_check(state: &RwLock<ProxyState>) {
    let state_r = state.read().await;
    let mut interval = time::interval(time::Duration::from_secs(state_r.active_health_check_interval as u64));
    let len = state_r.upstream_addresses.len();
    drop(state_r);
    interval.tick().await;
    loop {
        interval.tick().await;
        for upstream_idx in 0..len {
            let state_r = state.read().await;
            let upstream_ip = &state_r.upstream_addresses[upstream_idx];
            let request = http::Request::builder()
                .method(http::Method::GET)
                .uri(&state_r.active_health_check_path)
                .header("Host", upstream_ip)
                .body(Vec::new())
                .unwrap();
            match TcpStream::connect(upstream_ip).await {
                Ok(mut conn) => {
                    if let Err(error) = request::write_to_stream(&request, &mut conn).await {
                        log::error!("Failed to send request to upstream {}: {}", upstream_ip, error);
                        drop(state_r);
                        continue;
                    }
                    let response = match response::read_from_stream(&mut conn, request.method()).await {
                        Ok(response) => response,
                        Err(error) => {
                            log::error!("Error reading response from server: {:?}", error);
                            if !state_r.upstream_address_flags[upstream_idx] {
                                drop(state_r);
                                continue;
                            }
                            drop(state_r);
                            {
                                let mut state_w = state.write().await;
                                state_w.upstream_address_flags[upstream_idx] = false;
                                state_w.upstream_address_alive_num -= 1;
                            }
                            continue;
                        }
                    };
                    match response.status().as_u16() {
                        200 => {
                            if state_r.upstream_address_flags[upstream_idx] {
                                drop(state_r);
                                continue;
                            }
                            drop(state_r);
                            {
                                let mut state_w = state.write().await;
                                state_w.upstream_address_flags[upstream_idx] = true;
                                state_w.upstream_address_alive_num += 1;
                            }
                        }
                        status @ _ => {
                            log::error!("Upstream server {} is not working: {}", upstream_ip, status);
                            if !state_r.upstream_address_flags[upstream_idx] {
                                drop(state_r);
                                continue;
                            }
                            drop(state_r);
                            {
                                let mut state_w = state.write().await;
                                state_w.upstream_address_flags[upstream_idx] = false;
                                state_w.upstream_address_alive_num -= 1;
                            }
                        }
                    }
                }
                Err(err) => {
                    log::error!("Failed to connect to upstream {}: {}", upstream_ip, err);
                    if !state_r.upstream_address_flags[upstream_idx] {
                        drop(state_r);
                        continue;
                    }
                    drop(state_r);
                    {
                        let mut state_w = state.write().await;
                        state_w.upstream_address_flags[upstream_idx] = false;
                        state_w.upstream_address_alive_num -= 1;
                    }
                }
            }
        }
    }
}

async fn rate_limiting_counter_clear(state: &RwLock<ProxyState>) {
    let mut interval = time::interval(time::Duration::from_secs(60));
    interval.tick().await;
    loop {
        interval.tick().await;
        state.write().await.rate_limiting_counter.clear();
    }
}

async fn rate_limiting_check(state: &RwLock<ProxyState>, client_ip: &String) -> Result<(), std::io::Error> {
    if state.read().await.max_requests_per_minute == 0 {
        return Ok(());
    }
    let mut state_w = state.write().await;
    let count = state_w.rate_limiting_counter.entry(client_ip.to_string()).or_insert(0);
    *count += 1;
    if *count > state_w.max_requests_per_minute {
        Err(std::io::Error::new(std::io::ErrorKind::Other, "Too many requests"))
    } else {
        Ok(())
    }
}