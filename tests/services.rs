//! The real transport, against a one-shot server on the loopback interface.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread::JoinHandle;

use stk::services::{HttpTransport, ServiceHttp};

/// Answer one request with `status` and a JSON `body`, and hand back where.
fn serve_once(status: &'static str, body: &'static str) -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let url = format!("http://{}/2.0/", listener.local_addr().expect("an address"));
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("a request");
        let mut request = [0_u8; 4096];
        let _ = stream.read(&mut request);
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
             Connection: close\r\n\r\n{body}",
            body.len()
        );
        stream
            .write_all(response.as_bytes())
            .expect("a written response");
    });
    (url, server)
}

#[test]
fn a_refusal_is_reported_in_the_services_own_words() {
    let (url, server) = serve_once(
        "403 Forbidden",
        r#"{"error":10,"message":"Invalid API key - You must be granted a valid key by last.fm"}"#,
    );
    let http = HttpTransport::new(5.0, "").expect("a transport");

    let error = http
        .get_json(&url, &[("method", "user.getlovedtracks".to_owned())])
        .expect_err("a refusal");
    server.join().expect("the server");

    let message = error.to_string();
    assert!(message.contains("403"), "{message}");
    assert!(message.contains("Invalid API key"), "{message}");
}

#[test]
fn a_refusal_without_a_message_still_names_its_status() {
    let (url, server) = serve_once("404 Not Found", "{}");
    let http = HttpTransport::new(5.0, "").expect("a transport");

    let error = http.get_json(&url, &[]).expect_err("a refusal");
    server.join().expect("the server");

    assert!(error.to_string().contains("404 Not Found"), "{error}");
}

#[test]
fn a_success_is_read_as_json() {
    let (url, server) = serve_once("200 OK", r#"{"lovedtracks":{"track":[]}}"#);
    let http = HttpTransport::new(5.0, "").expect("a transport");

    let body = http.get_json(&url, &[]).expect("a body");
    server.join().expect("the server");

    assert!(body.get("lovedtracks").is_some());
}
