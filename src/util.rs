use tokio::sync::mpsc;

use eyre::Result;

pub(crate) async fn send<T: Send + Sync + 'static>(
    who: &str,
    sender: &mpsc::Sender<T>,
    item: T,
) -> Result<()> {
    let water_mark = sender.capacity();
    log::debug!("send {water_mark} {who}");
    let permit = sender.try_reserve();
    match permit {
        Ok(permit) => Ok(permit.send(item)),
        Err(err) => {
            log::warn!("unable to get permit for {who} {err} blocking");
            Ok(sender.send(item).await?)
        }
    }
}
