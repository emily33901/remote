use telemetry::server;

#[tokio::main]
async fn main() {
    let mut stream = server::stream().await;

    while let Some((id, event)) = stream.recv().await {
        log::info!("{id} {event:?}");
    }
}
