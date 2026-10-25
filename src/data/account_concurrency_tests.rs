use super::account_tests::{config, key, open, user};
use super::*;
use crate::auth::memory::InMemoryCredentialStore;
use crate::network::transport::MockTransport;
use std::sync::{
    Condvar, Mutex,
    atomic::{AtomicBool, Ordering},
};

#[derive(Default)]
struct PausedSave {
    inner: InMemoryCredentialStore,
    pause: AtomicBool,
    entered: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    released: Mutex<bool>,
    wake: Condvar,
}
impl CredentialStore for PausedSave {
    fn save(&self, r: &CredentialRef, p: &Pat) -> Result<(), CredentialError> {
        if self.pause.swap(false, Ordering::SeqCst) {
            if let Some(tx) = self.entered.lock().unwrap().take() {
                let _ = tx.send(());
            }
            let released = self.released.lock().unwrap();
            let (released, timeout) = self
                .wake
                .wait_timeout_while(released, std::time::Duration::from_secs(5), |v| !*v)
                .unwrap();
            assert!(
                *released && !timeout.timed_out(),
                "test failed to release save"
            );
        }
        self.inner.save(r, p)
    }
    fn load(&self, r: &CredentialRef) -> Result<Option<Pat>, CredentialError> {
        self.inner.load(r)
    }
    fn delete(&self, r: &CredentialRef) -> Result<(), CredentialError> {
        self.inner.delete(r)
    }
    fn describe(&self) -> &'static str {
        "paused test save"
    }
}
impl PausedSave {
    fn release(&self) {
        *self.released.lock().unwrap() = true;
        self.wake.notify_all();
    }
}

#[tokio::test]
async fn retired_credential_load_missing_or_error_is_stale_but_current_failure_is_preserved() {
    struct PausedLoad {
        inner: InMemoryCredentialStore,
        entered: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
        resume: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
        error: bool,
    }
    impl CredentialStore for PausedLoad {
        fn save(&self, r: &CredentialRef, p: &Pat) -> Result<(), CredentialError> {
            self.inner.save(r, p)
        }
        fn delete(&self, r: &CredentialRef) -> Result<(), CredentialError> {
            self.inner.delete(r)
        }
        fn describe(&self) -> &'static str {
            "paused load"
        }
        fn load(&self, r: &CredentialRef) -> Result<Option<Pat>, CredentialError> {
            let resume = self.resume.lock().unwrap().take();
            if let Some(resume) = resume {
                if let Some(tx) = self.entered.lock().unwrap().take() {
                    let _ = tx.send(());
                }
                resume
                    .recv_timeout(std::time::Duration::from_secs(5))
                    .unwrap();
                if self.error {
                    return Err(CredentialError::Corrupted);
                }
            }
            self.inner.load(r)
        }
    }
    for error in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let cfg = config(&temp);
        let store = Arc::new(PausedLoad {
            inner: InMemoryCredentialStore::new(),
            entered: Mutex::new(None),
            resume: Mutex::new(None),
            error,
        });
        let mock = Arc::new(MockTransport::new());
        let exec = Arc::new(BackgroundExecutor::new(2));
        let a = open(cfg, store.clone(), mock.clone(), exec).await;
        user(&mock, 1).await;
        a.login(Pat::from_str("old_fake")).await.unwrap();
        let (entered, ready) = tokio::sync::oneshot::channel();
        let (release, resume) = std::sync::mpsc::channel();
        *store.entered.lock().unwrap() = Some(entered);
        *store.resume.lock().unwrap() = Some(resume);
        let repos = a.repo_service();
        let read = tokio::spawn(async move { repos.list_repos().await });
        ready.await.unwrap();
        user(&mock, 1).await;
        a.replace(key(1), Pat::from_str("new_fake")).await.unwrap();
        release.send(()).unwrap();
        assert_eq!(read.await.unwrap(), Err(GitHubError::StaleContext));
        let reference = a.registry.lock().unwrap().credential_ref(key(1)).unwrap();
        store.delete(&reference).unwrap();
        assert!(
            matches!(
                a.repo_service().list_repos().await,
                Err(GitHubError::CredentialStore { .. })
            ),
            "current missing credential must remain a real error"
        );
        a.close().await.unwrap();
    }
}

#[tokio::test]
async fn aborting_commit_waiter_does_not_block_network_or_lose_commit_during_close() {
    let temp = tempfile::tempdir().unwrap();
    let cfg = config(&temp);
    let store = Arc::new(PausedSave::default());
    let mock = Arc::new(MockTransport::new());
    let exec = Arc::new(BackgroundExecutor::new(2));
    let a = Arc::new(open(cfg.clone(), store.clone(), mock.clone(), exec.clone()).await);
    let (tx, rx) = tokio::sync::oneshot::channel();
    *store.entered.lock().unwrap() = Some(tx);
    store.pause.store(true, Ordering::SeqCst);
    user(&mock, 1).await;
    let svc = a.clone();
    let login = tokio::spawn(async move { svc.login(Pat::from_str("fake")).await });
    rx.await.unwrap(); // exact boundary: SQLite staging committed; save blocked.
    tokio::time::timeout(std::time::Duration::from_millis(200), async {
        tokio::task::yield_now().await
    })
    .await
    .expect("network runtime responsive");
    login.abort();
    let _ = login.await;
    let svc = a.clone();
    let close = tokio::spawn(async move { svc.close().await });
    tokio::task::yield_now().await;
    assert!(!close.is_finished(), "close must drain the active commit");
    store.release();
    close.await.unwrap().unwrap();
    let b = open(cfg, store, mock, exec).await;
    assert_eq!(b.summaries().len(), 1);
    assert_eq!(b.active_context().unwrap().account, key(1));
    b.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_replace_remove_add_and_selection_publish_one_durable_directory() {
    let temp = tempfile::tempdir().unwrap();
    let cfg = config(&temp);
    let store = Arc::new(PausedSave::default());
    let mock = Arc::new(MockTransport::new());
    let exec = Arc::new(BackgroundExecutor::new(4));
    let a = Arc::new(open(cfg.clone(), store.clone(), mock.clone(), exec.clone()).await);
    for id in [1, 2] {
        user(&mock, id).await;
        a.login(Pat::from_str("fake")).await.unwrap();
    }
    let (tx, rx) = tokio::sync::oneshot::channel();
    *store.entered.lock().unwrap() = Some(tx);
    store.pause.store(true, Ordering::SeqCst);
    user(&mock, 1).await;
    let svc = a.clone();
    let replacement =
        tokio::spawn(async move { svc.replace(key(1), Pat::from_str("replacement_fake")).await });
    rx.await.unwrap();
    user(&mock, 3).await;
    let svc = a.clone();
    let add = tokio::spawn(async move { svc.login(Pat::from_str("third_fake")).await });
    let svc = a.clone();
    let remove = tokio::spawn(async move { svc.remove_saved(key(1)).await });
    let svc = a.clone();
    let select = tokio::spawn(async move { svc.set_active(key(2)).await });
    store.release();
    replacement.await.unwrap().unwrap();
    add.await.unwrap().unwrap();
    remove.await.unwrap().unwrap();
    select.await.unwrap().unwrap();
    let expected = a
        .summaries()
        .into_iter()
        .map(|s| s.identity.key)
        .collect::<Vec<_>>();
    let active = a.active_context().unwrap().account;
    assert_eq!(expected.len(), 2);
    assert!(!expected.contains(&key(1)));
    a.close().await.unwrap();
    let b = open(cfg, store, mock, exec).await;
    assert_eq!(
        b.summaries()
            .into_iter()
            .map(|s| s.identity.key)
            .collect::<Vec<_>>(),
        expected
    );
    assert_eq!(b.active_context().unwrap().account, active);
    b.close().await.unwrap();
}
