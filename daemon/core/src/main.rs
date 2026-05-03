mod error;
mod http;

#[tokio::main]
async fn main() {
    http::launch_server().await.unwrap();
}
