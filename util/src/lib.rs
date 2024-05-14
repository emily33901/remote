use std::{collections::HashSet, future::Future, thread::yield_now};

use tokio::{
    pin,
    task::{JoinError, JoinHandle},
};

pub trait JoinhandleExt<T> {
    fn watch<F: FnOnce(T) -> () + Send + 'static>(self, f: F) -> ();
}

impl<T: Send + 'static> JoinhandleExt<T> for JoinHandle<T> {
    fn watch<F: FnOnce(T) -> () + Send + 'static>(self, f: F) -> () {
        tokio::spawn(async move {
            let r = self.await.unwrap();
            f(r);
        });
    }
}
