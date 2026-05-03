mod error;
mod http;
mod session;

#[tokio::main]
async fn main() {
    http::launch_server().await.unwrap();
}
