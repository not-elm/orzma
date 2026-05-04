mod error;
mod http;
mod macros;
mod pty;
mod session;

#[tokio::main]
async fn main() {
    http::launch_server().await.unwrap();
}
