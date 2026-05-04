mod error;
mod http;
mod macros;
mod session;

#[tokio::main]
async fn main() {
    http::launch_server().await.unwrap();
}
