use serde_json::{Value, json};
use std::{
    future::{Future, poll_fn},
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    task::Poll,
    time::Duration,
};
use switchbite::runtime::{DecisionRuntime, RuntimeConfig, RuntimeError};
use tokio::{sync::oneshot, task::JoinHandle};

fn config(concurrent: usize, queued: usize) -> RuntimeConfig {
    RuntimeConfig {
        max_concurrent: concurrent,
        max_queued: queued,
        queue_timeout_ms: 100,
        decision_timeout_ms: 2_000,
    }
}

async fn pending<F: Future>(mut future: Pin<&mut F>) {
    poll_fn(|cx| {
        assert!(
            future.as_mut().poll(cx).is_pending(),
            "request should be queued"
        );
        Poll::Ready(())
    })
    .await;
}

async fn blocked(
    runtime: &DecisionRuntime,
) -> (JoinHandle<Result<Value, RuntimeError>>, mpsc::Sender<()>) {
    let (started_tx, started_rx) = oneshot::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let runtime = runtime.clone();
    let job = tokio::spawn(async move {
        runtime
            .execute(move || {
                let _ = started_tx.send(());
                release_rx.recv().unwrap();
                Ok(json!({"proposal": true}))
            })
            .await
    });
    started_rx.await.unwrap();
    (job, release_tx)
}

async fn available(runtime: &DecisionRuntime) {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match runtime.execute(|| Ok(json!(true))).await {
                Ok(_) => break,
                Err(error) if error.code == "runtime_busy" => tokio::task::yield_now().await,
                result => panic!("unexpected result: {result:?}"),
            }
        }
    })
    .await
    .expect("finished work must release its permits");
}

#[test]
fn defaults_caps_and_safe_errors() {
    fn send_sync_clone<T: Send + Sync + Clone>() {}
    send_sync_clone::<DecisionRuntime>();
    assert_eq!(
        RuntimeConfig::default(),
        RuntimeConfig {
            max_concurrent: 2,
            max_queued: 16,
            queue_timeout_ms: 5_000,
            decision_timeout_ms: 30_000
        }
    );
    for invalid in [
        RuntimeConfig {
            max_concurrent: 0,
            ..config(1, 0)
        },
        RuntimeConfig {
            max_concurrent: 65,
            ..config(1, 0)
        },
        RuntimeConfig {
            max_queued: usize::MAX,
            ..config(1, 0)
        },
        RuntimeConfig {
            queue_timeout_ms: 0,
            ..config(1, 0)
        },
        RuntimeConfig {
            decision_timeout_ms: u64::MAX,
            ..config(1, 0)
        },
    ] {
        assert_eq!(
            DecisionRuntime::new(invalid).err().unwrap().code,
            "invalid_runtime_config"
        );
    }
    let mut error = RuntimeError::from_code("provider_rate_limited");
    error.message = "untrusted upstream body";
    error.retry_after_ms = Some(u64::MAX);
    assert_eq!(error.to_value()["retry_after_ms"], 30_000);
    assert!(!error.to_value().to_string().contains("untrusted"));
    assert!(error.to_value()["next_action"].is_string());
    assert_eq!(
        RuntimeError::from_code("untrusted provider text").code,
        "runtime_failed"
    );
}

#[tokio::test]
async fn actual_overlap_and_bounded_queue_across_clones() {
    let runtime = DecisionRuntime::new(config(2, 1)).unwrap();
    let active = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let mut jobs = Vec::new();
    let mut releases = Vec::new();
    for _ in 0..2 {
        let (started_tx, started_rx) = oneshot::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (runtime, active, peak) = (runtime.clone(), active.clone(), peak.clone());
        jobs.push(tokio::spawn(async move {
            runtime
                .execute(move || {
                    let count = active.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(count, Ordering::SeqCst);
                    let _ = started_tx.send(());
                    release_rx.recv().unwrap();
                    active.fetch_sub(1, Ordering::SeqCst);
                    Ok(json!(true))
                })
                .await
        }));
        started_rx.await.unwrap();
        releases.push(release_tx);
    }
    assert_eq!(active.load(Ordering::SeqCst), 2);
    let dispatched = Arc::new(AtomicBool::new(false));
    let marker = dispatched.clone();
    let mut queued = Box::pin(runtime.execute(move || {
        marker.store(true, Ordering::SeqCst);
        Ok(json!({"queued": true}))
    }));
    pending(queued.as_mut()).await;
    assert!(!dispatched.load(Ordering::SeqCst));
    assert_eq!(
        runtime
            .execute(|| panic!("queue exceeded"))
            .await
            .unwrap_err()
            .code,
        "runtime_busy"
    );
    releases[0].send(()).unwrap();
    assert_eq!(queued.await.unwrap()["queued"], true);
    releases[1].send(()).unwrap();
    for job in jobs {
        job.await.unwrap().unwrap();
    }
    assert_eq!(peak.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn queue_timeout_and_drop_never_dispatch_later() {
    let runtime = DecisionRuntime::new(RuntimeConfig {
        queue_timeout_ms: 30,
        ..config(1, 1)
    })
    .unwrap();
    let (job, release) = blocked(&runtime).await;
    let error = runtime
        .execute(|| panic!("timed out queue dispatched"))
        .await
        .unwrap_err();
    assert_eq!(error.code, "queue_timeout");
    let mut queued = Box::pin(runtime.execute(|| panic!("cancelled queue dispatched")));
    pending(queued.as_mut()).await;
    drop(queued);
    // Dropping a waiter must immediately free its queue/admission slot.
    let mut replacement = Box::pin(runtime.execute(|| Ok(json!("replacement"))));
    pending(replacement.as_mut()).await;
    release.send(()).unwrap();
    job.await.unwrap().unwrap();
    assert_eq!(replacement.await.unwrap(), "replacement");
}

#[tokio::test]
async fn caller_cancellation_keeps_running_permits_until_exit() {
    let runtime = DecisionRuntime::new(config(1, 0)).unwrap();
    let (job, release) = blocked(&runtime).await;
    job.abort();
    assert!(job.await.unwrap_err().is_cancelled());
    assert_eq!(
        runtime
            .execute(|| panic!("running slot released early"))
            .await
            .unwrap_err()
            .code,
        "runtime_busy"
    );
    release.send(()).unwrap();
    available(&runtime).await;
}

#[tokio::test]
async fn caller_timeout_keeps_running_permits_until_exit() {
    let runtime = DecisionRuntime::new(RuntimeConfig {
        decision_timeout_ms: 50,
        ..config(1, 0)
    })
    .unwrap();
    let (job, release) = blocked(&runtime).await;
    assert_eq!(job.await.unwrap().unwrap_err().code, "decision_timeout");
    assert_eq!(
        runtime
            .execute(|| panic!("timed out work still running"))
            .await
            .unwrap_err()
            .code,
        "runtime_busy"
    );
    release.send(()).unwrap();
    available(&runtime).await;
}

#[tokio::test]
async fn total_deadline_can_expire_while_queued() {
    let runtime = DecisionRuntime::new(RuntimeConfig {
        decision_timeout_ms: 50,
        queue_timeout_ms: 1_000,
        ..config(1, 1)
    })
    .unwrap();
    let (job, release) = blocked(&runtime).await;
    assert_eq!(
        runtime
            .execute(|| panic!("total deadline ignored in queue"))
            .await
            .unwrap_err()
            .code,
        "decision_timeout"
    );
    release.send(()).unwrap();
    assert_eq!(job.await.unwrap().unwrap_err().code, "decision_timeout");
    available(&runtime).await;
}

#[tokio::test]
async fn queue_wait_consumes_execution_budget() {
    let runtime = DecisionRuntime::new(RuntimeConfig {
        queue_timeout_ms: 500,
        decision_timeout_ms: 240,
        ..config(1, 1)
    })
    .unwrap();
    let (first, release_first) = blocked(&runtime).await;
    let (release_work, wait_work) = mpsc::channel();
    let (started_tx, started_rx) = oneshot::channel();
    let mut queued = Box::pin(runtime.execute(move || {
        let _ = started_tx.send(());
        wait_work.recv().unwrap();
        Ok(json!(true))
    }));
    pending(queued.as_mut()).await;
    tokio::time::sleep(Duration::from_millis(120)).await;
    release_first.send(()).unwrap();
    first.await.unwrap().unwrap();
    // Poll the queued request until the work starts, then measure only the remaining budget.
    tokio::select! {
        result = &mut queued => panic!("finished before release: {result:?}"),
        result = started_rx => result.unwrap(),
    }
    let result = tokio::time::timeout(Duration::from_millis(200), &mut queued).await;
    release_work.send(()).unwrap();
    assert_eq!(
        result
            .expect("queue wait must count toward the deadline")
            .unwrap_err()
            .code,
        "decision_timeout"
    );
    available(&runtime).await;
}

#[tokio::test]
async fn panic_and_work_errors_release_permits() {
    let runtime = DecisionRuntime::new(config(1, 0)).unwrap();
    assert_eq!(
        runtime
            .execute(|| panic!("fixture panic"))
            .await
            .unwrap_err()
            .code,
        "runtime_failed"
    );
    let failure = RuntimeError::from_code("provider_auth_failed");
    assert_eq!(
        runtime.execute(move || Err(failure)).await.unwrap_err(),
        failure
    );
    available(&runtime).await;
}

#[test]
fn cancelling_before_blocking_dispatch_never_runs_the_closure() {
    let executor = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .max_blocking_threads(1)
        .build()
        .unwrap();
    executor.block_on(async {
        let runtime = DecisionRuntime::new(config(1, 0)).unwrap();
        let (started_tx, started_rx) = oneshot::channel();
        let (release, wait) = mpsc::channel();
        let blocker = tokio::task::spawn_blocking(move || {
            let _ = started_tx.send(());
            let _ = wait.recv();
        });
        started_rx.await.unwrap();
        let dispatched = Arc::new(AtomicBool::new(false));
        let marker = dispatched.clone();
        let mut future = Box::pin(runtime.execute(move || {
            marker.store(true, Ordering::SeqCst);
            Ok(json!(true))
        }));
        pending(future.as_mut()).await;
        drop(future);
        release.send(()).unwrap();
        blocker.await.unwrap();
        available(&runtime).await;
        assert!(!dispatched.load(Ordering::SeqCst));
    });
}
