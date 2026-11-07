mod common;

use common::{pattern, Env, Mode, TestServer};
use std::time::{Duration, Instant};
use unduhkilat_core::engine::manager::NewDownload;
use unduhkilat_core::error::ErrorCode;
use unduhkilat_core::models::DownloadState;
use unduhkilat_core::repo;

const T: Duration = Duration::from_secs(30);

fn req(url: String, name: &str, env: &Env) -> NewDownload {
    NewDownload {
        url,
        file_name: Some(name.into()),
        target_path: Some(env.path(name)),
        start_immediately: true,
        ..Default::default()
    }
}

fn assert_file_eq(env: &Env, name: &str, expected: &[u8]) {
    let got = std::fs::read(env.path(name)).unwrap();
    assert_eq!(got.len(), expected.len(), "length");
    assert!(got == expected, "content differs");
}

// ---- TASK-06/07/09: probe, segmented transfer, verification -----------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn segmented_download_is_byte_identical_with_configured_connections() {
    let srv = TestServer::start(3_000_001, Mode::Ranges).await;
    let env = Env::new().await;
    let mut r = req(srv.url("/file"), "a.bin", &env);
    r.connection_count = Some(8);
    let dl = env.manager.create(r).await.unwrap();
    let done = env.manager.wait_for_state(&dl.id, DownloadState::Completed, T).await.unwrap();

    assert_file_eq(&env, "a.bin", &srv.state.data);
    assert!(done.segmentation_supported);
    assert_eq!(done.downloaded_bytes, 3_000_001);
    assert_eq!(done.total_bytes, 3_000_001);
    assert!(done.completed_at.is_some());

    let segs = env.manager.db().read(|c| repo::get_segments(c, &dl.id)).unwrap();
    assert_eq!(segs.len(), 8);
    for s in &segs {
        assert_eq!(s.highest_contiguous_offset, s.range_end + 1, "completed => range_end + 1");
    }
    let ranged: Vec<_> = srv.log().into_iter().filter_map(|l| l.range).filter(|r| r != "bytes=0-0").collect();
    assert_eq!(ranged.len(), 8, "exactly one range request per configured connection: {ranged:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dec001_no_ceiling_on_connections_per_download() {
    let srv = TestServer::start(1_000_000, Mode::Ranges).await;
    let env = Env::new().await;
    let mut r = req(srv.url("/file"), "many.bin", &env);
    r.connection_count = Some(64);
    let dl = env.manager.create(r).await.unwrap();
    env.manager.wait_for_state(&dl.id, DownloadState::Completed, T).await.unwrap();
    assert_eq!(env.manager.db().read(|c| repo::get_segments(c, &dl.id)).unwrap().len(), 64);
    assert_file_eq(&env, "many.bin", &srv.state.data);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn connection_count_zero_is_rejected() {
    let srv = TestServer::start(10, Mode::Ranges).await;
    let env = Env::new().await;
    let mut r = req(srv.url("/file"), "z.bin", &env);
    r.connection_count = Some(0);
    assert_eq!(env.manager.create(r).await.unwrap_err().code, ErrorCode::ErrInvalidArgument);
}

// ---- TASK-08: single-connection fallback -------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn server_without_ranges_falls_back_to_one_connection() {
    let srv = TestServer::start(500_000, Mode::NoRanges).await;
    let env = Env::new().await;
    let mut r = req(srv.url("/file"), "nr.bin", &env);
    r.connection_count = Some(8);
    let dl = env.manager.create(r).await.unwrap();
    let done = env.manager.wait_for_state(&dl.id, DownloadState::Completed, T).await.unwrap();

    assert!(!done.segmentation_supported, "list must show segmentation was unavailable (FR-005)");
    assert_file_eq(&env, "nr.bin", &srv.state.data);
    let gets = srv.log().iter().filter(|l| l.path == "/file").count();
    assert_eq!(gets, 2, "one probe + exactly one transfer connection");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unknown_length_stream_completes_and_records_total() {
    let srv = TestServer::start(300_000, Mode::Unknown).await;
    let env = Env::new().await;
    let dl = env.manager.create(req(srv.url("/file"), "u.bin", &env)).await.unwrap();
    let done = env.manager.wait_for_state(&dl.id, DownloadState::Completed, T).await.unwrap();
    assert_eq!(done.total_bytes, 300_000);
    assert_file_eq(&env, "u.bin", &srv.state.data);
}

// ---- TASK-06 / TASK-19: redirect + header context ----------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn redirect_is_followed_and_referer_cookie_useragent_are_preserved() {
    let srv = TestServer::start(200_000, Mode::Ranges).await;
    let env = Env::new().await;
    let mut r = req(srv.url("/redirect"), "r.bin", &env);
    r.referer = Some("http://origin.test/page".into());
    r.cookies = Some("sid=abc123".into());
    r.user_agent = Some("TestAgent/9".into());
    r.connection_count = Some(4);
    let dl = env.manager.create(r).await.unwrap();
    let done = env.manager.wait_for_state(&dl.id, DownloadState::Completed, T).await.unwrap();

    assert!(done.effective_url.ends_with("/file"), "effective url recorded: {}", done.effective_url);
    assert_file_eq(&env, "r.bin", &srv.state.data);
    let file_reqs: Vec<_> = srv.log().into_iter().filter(|l| l.path == "/file").collect();
    assert!(file_reqs.len() >= 5);
    for l in file_reqs {
        assert_eq!(l.referer.as_deref(), Some("http://origin.test/page"), "referer on every request");
        assert_eq!(l.cookie.as_deref(), Some("sid=abc123"), "cookie on every request");
        assert_eq!(l.user_agent.as_deref(), Some("TestAgent/9"));
    }
}

// ---- TASK-14/15/17/18: pause, resume, restart, crash recovery -----------------

async fn wait_progress(env: &Env, id: &str, at_least: i64) {
    let t = Instant::now();
    loop {
        if env.manager.get(id).unwrap().downloaded_bytes >= at_least {
            return;
        }
        assert!(t.elapsed() < T, "no progress");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pause_then_resume_requests_exactly_the_persisted_next_byte() {
    let srv = TestServer::start(2_000_000, Mode::Ranges).await;
    srv.set_delay_ms(8);
    let env = Env::new().await;
    let mut r = req(srv.url("/file"), "p.bin", &env);
    r.connection_count = Some(4);
    let dl = env.manager.create(r).await.unwrap();

    wait_progress(&env, &dl.id, 300_000).await;
    assert_eq!(env.manager.pause(&dl.id).await.unwrap(), DownloadState::Paused);
    let paused = env.manager.wait_for_state(&dl.id, DownloadState::Paused, T).await.unwrap();
    assert!(paused.downloaded_bytes > 0 && paused.downloaded_bytes < 2_000_000);

    let segs = env.manager.db().read(|c| repo::get_segments(c, &dl.id)).unwrap();
    assert!(segs.iter().any(|s| s.highest_contiguous_offset > s.range_start), "progress was checkpointed");
    for s in &segs {
        assert!(s.range_start <= s.highest_contiguous_offset && s.highest_contiguous_offset <= s.range_end + 1);
    }

    srv.clear_log();
    srv.set_delay_ms(0);
    env.manager.resume(&dl.id).await.unwrap();
    env.manager.wait_for_state(&dl.id, DownloadState::Completed, T).await.unwrap();
    assert_file_eq(&env, "p.bin", &srv.state.data);

    // Every incomplete segment must have been re-requested from EXACTLY its persisted offset.
    let ranges: Vec<String> = srv.log().into_iter().filter_map(|l| l.range).collect();
    for s in &segs {
        let want = format!("bytes={}-{}", s.highest_contiguous_offset, s.range_end);
        if s.highest_contiguous_offset <= s.range_end {
            assert!(ranges.contains(&want), "missing resume request {want}; saw {ranges:?}");
        } else {
            assert!(!ranges.iter().any(|r| r.starts_with(&format!("bytes={}-", s.range_start)) && r.ends_with(&format!("-{}", s.range_end))), "completed segment {} must not be requested again", s.segment_index);
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn restart_discards_progress_and_downloads_from_zero() {
    let srv = TestServer::start(1_500_000, Mode::Ranges).await;
    srv.set_delay_ms(8);
    let env = Env::new().await;
    let mut r = req(srv.url("/file"), "rs.bin", &env);
    r.connection_count = Some(3);
    let dl = env.manager.create(r).await.unwrap();
    wait_progress(&env, &dl.id, 200_000).await;
    env.manager.pause(&dl.id).await.unwrap();
    env.manager.wait_for_state(&dl.id, DownloadState::Paused, T).await.unwrap();

    srv.clear_log();
    srv.set_delay_ms(0);
    env.manager.restart(&dl.id).await.unwrap();
    env.manager.wait_for_state(&dl.id, DownloadState::Completed, T).await.unwrap();
    assert_file_eq(&env, "rs.bin", &srv.state.data);
    let ranges: Vec<String> = srv.log().into_iter().filter_map(|l| l.range).collect();
    assert!(ranges.iter().any(|r| r.starts_with("bytes=0-") && r != "bytes=0-0"), "first segment restarted at byte 0: {ranges:?}");
    assert_eq!(env.manager.get(&dl.id).unwrap().downloaded_bytes, 1_500_000);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn crash_recovery_normalizes_running_to_paused_and_resumes_byte_identical() {
    let srv = TestServer::start(2_000_000, Mode::Ranges).await;
    srv.set_delay_ms(8);
    let env = Env::new().await;
    let mut r = req(srv.url("/file"), "c.bin", &env);
    r.connection_count = Some(4);
    let dl = env.manager.create(r).await.unwrap();
    wait_progress(&env, &dl.id, 300_000).await;
    env.manager.pause(&dl.id).await.unwrap();
    env.manager.wait_for_state(&dl.id, DownloadState::Paused, T).await.unwrap();

    // Recreate the on-disk state a TerminateProcess would leave behind:
    // the row says `running`, segments say `downloading`, offsets are the last checkpoint.
    let before = env.manager.db().read(|c| repo::get_segments(c, &dl.id)).unwrap();
    let id = dl.id.clone();
    env.manager
        .db()
        .write(move |c| {
            c.execute("UPDATE downloads SET state='running' WHERE id=?1", [&id])?;
            c.execute("UPDATE segments SET state='downloading' WHERE download_id=?1 AND state<>'completed'", [&id])?;
            Ok(())
        })
        .await
        .unwrap();

    // "Next launch": a fresh Db handle + Manager over the same file.
    srv.set_delay_ms(0);
    let db2 = unduhkilat_core::db::Db::open(&env.db_path).unwrap();
    let mut cfg = unduhkilat_core::config::EngineConfig::default();
    cfg.checkpoint_interval = Duration::from_millis(40);
    let m2 = unduhkilat_core::engine::manager::Manager::start(db2, cfg).await.unwrap();

    let got = m2.get(&dl.id).unwrap();
    assert_eq!(got.state, DownloadState::Paused, "running => paused before any dispatch");
    assert_eq!(m2.running_count(), 0, "nothing scheduled by recovery");
    let after = m2.db().read(|c| repo::get_segments(c, &dl.id)).unwrap();
    for (b, a) in before.iter().zip(&after) {
        assert_eq!(b.highest_contiguous_offset, a.highest_contiguous_offset, "offsets intact");
        assert_ne!(a.state, unduhkilat_core::models::SegmentState::Downloading, "orphaned segments reset");
    }

    m2.start_all().await.unwrap(); // one-action resume of everything
    m2.wait_for_state(&dl.id, DownloadState::Completed, T).await.unwrap();
    assert_file_eq(&env, "c.bin", &srv.state.data);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dropped_connection_fails_with_code_keeps_progress_and_resumes() {
    // No automatic retry exists yet (retry policy is TBD-006/007): the item must
    // fail visibly, keep its checkpoints, and finish after a manual resume.
    let srv = TestServer::start(400_000, Mode::Ranges).await;
    let env = Env::new().await;
    let mut r = req(srv.url("/file"), "d.bin", &env);
    r.connection_count = Some(2);
    srv.state.truncate_budget.store(1, std::sync::atomic::Ordering::SeqCst);
    let dl = env.manager.create(r).await.unwrap();
    let failed = env.manager.wait_for_state(&dl.id, DownloadState::Failed, T).await.unwrap();
    assert_eq!(failed.error_code.as_deref(), Some("ERR_CONNECTION_DROPPED"));
    let segs = env.manager.db().read(|c| repo::get_segments(c, &dl.id)).unwrap();
    assert!(segs.iter().any(|s| s.highest_contiguous_offset > s.range_start), "bytes before the drop were kept");

    env.manager.resume(&dl.id).await.unwrap();
    env.manager.wait_for_state(&dl.id, DownloadState::Completed, T).await.unwrap();
    assert_file_eq(&env, "d.bin", &srv.state.data);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancel_with_delete_removes_file_and_queue_entry_and_is_idempotent() {
    let srv = TestServer::start(1_000_000, Mode::Ranges).await;
    srv.set_delay_ms(8);
    let env = Env::new().await;
    let dl = env.manager.create(req(srv.url("/file"), "x.bin", &env)).await.unwrap();
    wait_progress(&env, &dl.id, 50_000).await;
    env.manager.cancel(&dl.id, true).await.unwrap();
    env.manager.wait_for_state(&dl.id, DownloadState::Cancelled, T).await.unwrap();
    assert!(!env.path("x.bin").exists());
    assert!(env.manager.db().read(|c| repo::get_queue_item(c, &dl.id)).is_err());
    assert_eq!(env.manager.cancel(&dl.id, true).await.unwrap(), DownloadState::Cancelled);
    assert_eq!(env.manager.pause(&dl.id).await.unwrap(), DownloadState::Cancelled, "pause on cancelled is a no-op");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn delete_and_clear_finished_remove_records_but_keep_files_by_default() {
    let srv = TestServer::start(50_000, Mode::Ranges).await;
    let env = Env::new().await;
    let a = env.manager.create(req(srv.url("/file"), "da.bin", &env)).await.unwrap();
    let b = env.manager.create(req(srv.url("/file"), "db.bin", &env)).await.unwrap();
    env.manager.wait_for_state(&a.id, DownloadState::Completed, T).await.unwrap();
    env.manager.wait_for_state(&b.id, DownloadState::Completed, T).await.unwrap();

    env.manager.delete(&a.id, false).await.unwrap();
    assert!(env.manager.get(&a.id).is_err());
    assert!(env.path("da.bin").exists(), "file kept");
    assert_eq!(env.manager.clear_finished().await.unwrap(), 1);
    assert!(env.path("db.bin").exists());

    srv.set_delay_ms(30);
    let c = env.manager.create(req(srv.url("/file"), "dc.bin", &env)).await.unwrap();
    wait_progress(&env, &c.id, 1000).await;
    env.manager.delete(&c.id, true).await.unwrap();
    assert!(!env.path("dc.bin").exists(), "running delete with file removes it");
    assert_eq!(env.manager.running_count(), 0);
}

// ---- TASK-12 (conservative part): never overwrite ---------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn existing_target_is_never_overwritten() {
    let srv = TestServer::start(1000, Mode::Ranges).await;
    let env = Env::new().await;
    std::fs::create_dir_all(env.path("e.bin").parent().unwrap()).unwrap();
    std::fs::write(env.path("e.bin"), b"precious").unwrap();
    let err = env.manager.create(req(srv.url("/file"), "e.bin", &env)).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::ErrNameCollision);
    assert_eq!(std::fs::read(env.path("e.bin")).unwrap(), b"precious");

    srv.set_delay_ms(50);
    let first = env.manager.create(req(srv.url("/file"), "f.bin", &env)).await.unwrap();
    let dup = env.manager.create(req(srv.url("/file"), "f.bin", &env)).await.unwrap_err();
    assert_eq!(dup.code, ErrorCode::ErrNameCollision, "two active downloads may not share a target");
    env.manager.cancel(&first.id, true).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unsupported_scheme_is_rejected() {
    let env = Env::new().await;
    let err = env.manager.create(NewDownload { url: "gopher://x/y".into(), target_path: Some(env.path("g")), start_immediately: true, ..Default::default() }).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::ErrInvalidUrlScheme);
}

// ---- categories routing (TASK-25) ------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn download_is_routed_to_its_category_folder_with_fallback() {
    let srv = TestServer::start(5000, Mode::Ranges).await;
    let env = Env::new().await;
    let iso = env.manager.create(NewDownload { url: srv.url("/file"), file_name: Some("ubuntu.iso".into()), start_immediately: true, ..Default::default() }).await.unwrap();
    assert!(iso.target_path.replace('/', "\\").ends_with("dl\\Images\\ubuntu.iso"), "{}", iso.target_path);
    let unk = env.manager.create(NewDownload { url: srv.url("/file"), file_name: Some("blob.zzz".into()), start_immediately: true, ..Default::default() }).await.unwrap();
    assert!(unk.target_path.replace('/', "\\").ends_with("dl\\General\\blob.zzz"), "{}", unk.target_path);
    env.manager.wait_for_state(&iso.id, DownloadState::Completed, T).await.unwrap();
    env.manager.wait_for_state(&unk.id, DownloadState::Completed, T).await.unwrap();
}

// ---- TASK-20/21/22: queue dispatch ------------------------------------------------

async fn staged(env: &Env, srv: &TestServer, specs: &[(&str, i32)]) -> Vec<String> {
    let mut ids = Vec::new();
    for (name, prio) in specs {
        let mut r = req(srv.url("/file"), name, env);
        r.start_immediately = false; // stage everything first so dispatch order is observable
        r.priority = *prio;
        r.connection_count = Some(2);
        ids.push(env.manager.create(r).await.unwrap().id);
    }
    ids
}

async fn start_order(env: &Env, n: usize) -> Vec<String> {
    let mut rx = env.manager.subscribe();
    env.manager.start_all().await.unwrap();
    let mut order = Vec::new();
    let t = Instant::now();
    while order.len() < n {
        assert!(t.elapsed() < T, "only saw {order:?}");
        if let Ok(Ok(unduhkilat_core::engine::EngineEvent::StateChanged { id, state: DownloadState::Running, .. })) = tokio::time::timeout(Duration::from_millis(200), rx.recv()).await {
            order.push(id);
        }
    }
    order
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dispatch_is_priority_first_then_fifo() {
    let srv = TestServer::start(100_000, Mode::Ranges).await;
    srv.set_delay_ms(2);
    let env = Env::new().await;
    env.manager.set_concurrency_limit(1).await.unwrap();
    let ids = staged(&env, &srv, &[("q0", 0), ("q1", 0), ("q2", 5), ("q3", 0), ("q4", 5)]).await;
    let order = start_order(&env, 5).await;
    assert_eq!(order, vec![ids[2].clone(), ids[4].clone(), ids[0].clone(), ids[1].clone(), ids[3].clone()]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn priority_change_and_reorder_take_effect_at_next_dispatch() {
    let srv = TestServer::start(100_000, Mode::Ranges).await;
    srv.set_delay_ms(2);
    let env = Env::new().await;
    env.manager.set_concurrency_limit(1).await.unwrap();
    let ids = staged(&env, &srv, &[("w0", 0), ("w1", 0), ("w2", 0), ("w3", 0)]).await;

    env.manager.set_priority(&ids[3], 9).await.unwrap(); // jumps ahead
    env.manager.reorder(vec![ids[2].clone(), ids[1].clone()]).await.unwrap(); // manual FIFO among equals
    let order = start_order(&env, 4).await;
    assert_eq!(order, vec![ids[3].clone(), ids[2].clone(), ids[1].clone(), ids[0].clone()]);

    let pos: Vec<u32> = ids.iter().map(|i| env.manager.db().read(|c| repo::get_queue_item(c, i)).unwrap().queue_position).collect();
    let mut sorted = pos.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted, vec![0, 1, 2, 3], "dense re-index: {pos:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrency_limit_is_exactly_honoured_and_has_no_ceiling() {
    let srv = TestServer::start(300_000, Mode::Ranges).await;
    srv.set_delay_ms(60); // each item takes ~600 ms, far longer than the sampling window
    let env = Env::new().await;
    env.manager.set_concurrency_limit(2).await.unwrap();
    let ids = staged(&env, &srv, &[("k0", 0), ("k1", 0), ("k2", 0), ("k3", 0), ("k4", 0), ("k5", 0)]).await;
    env.manager.start_all().await.unwrap();
    let (mut max, t) = (0, Instant::now());
    while t.elapsed() < Duration::from_millis(250) {
        max = max.max(env.manager.running_count());
        assert!(env.manager.running_count() <= 2, "limit exceeded");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(max, 2, "limit reached when enough items are queued");
    let queued = env.manager.db().read(|c| repo::list_downloads(c, Some(DownloadState::Queued))).unwrap().len();
    assert_eq!(queued, 4);

    // DEC-001: raise it to 50 and all remaining start together.
    srv.set_delay_ms(30);
    env.manager.set_concurrency_limit(50).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(env.manager.running_count() >= 4, "got {}", env.manager.running_count());
    srv.set_delay_ms(0);
    for id in &ids {
        env.manager.wait_for_state(id, DownloadState::Completed, T).await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stop_all_pauses_everything_and_start_all_resumes_within_the_limit() {
    let srv = TestServer::start(1_500_000, Mode::Ranges).await;
    srv.set_delay_ms(8);
    let env = Env::new().await;
    env.manager.set_concurrency_limit(2).await.unwrap();
    let ids = staged(&env, &srv, &[("s0", 0), ("s1", 0), ("s2", 0)]).await;
    env.manager.start_all().await.unwrap();
    wait_progress(&env, &ids[0], 100_000).await;
    env.manager.stop_all().await.unwrap();
    for id in &ids {
        env.manager.wait_for_state(id, DownloadState::Paused, T).await.unwrap();
    }
    assert_eq!(env.manager.running_count(), 0);
    let kept = env.manager.get(&ids[0]).unwrap().downloaded_bytes;
    assert!(kept >= 100_000, "progress preserved");

    srv.set_delay_ms(0);
    let (_, running) = env.manager.start_all().await.unwrap();
    assert!(running <= 2);
    for id in &ids {
        env.manager.wait_for_state(id, DownloadState::Completed, T).await.unwrap();
    }
}

// ---- TASK-24: bandwidth -----------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn global_limit_caps_aggregate_throughput() {
    let srv = TestServer::start(300 * 1024, Mode::Ranges).await;
    let env = Env::new().await;
    env.manager.set_global_speed_limit(200).await.unwrap(); // 200 KB/s
    let mut r = req(srv.url("/file"), "g.bin", &env);
    r.connection_count = Some(4);
    let t = Instant::now();
    let dl = env.manager.create(r).await.unwrap();
    env.manager.wait_for_state(&dl.id, DownloadState::Completed, T).await.unwrap();
    let secs = t.elapsed().as_secs_f64();
    assert!(secs >= 1.0, "300 KB at 200 KB/s cannot finish in {secs}s");
    assert!(secs < 6.0, "{secs}s");
    assert_file_eq(&env, "g.bin", &srv.state.data);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn per_download_limit_applies_and_can_be_lifted_live() {
    let srv = TestServer::start(2 * 1024 * 1024, Mode::Ranges).await;
    let env = Env::new().await;
    let mut r = req(srv.url("/file"), "l.bin", &env);
    r.speed_limit_kbps = 100; // 2 MB would take ~20 s
    r.connection_count = Some(2);
    let t = Instant::now();
    let dl = env.manager.create(r).await.unwrap();
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert!(env.manager.get(&dl.id).unwrap().downloaded_bytes < 600 * 1024, "limit not applied");
    env.manager.set_download_speed_limit(&dl.id, 0).await.unwrap(); // live, no pause/restart
    env.manager.wait_for_state(&dl.id, DownloadState::Completed, T).await.unwrap();
    assert!(t.elapsed() < Duration::from_secs(10), "live lift did not take effect: {:?}", t.elapsed());
    assert_file_eq(&env, "l.bin", &srv.state.data);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_checkpoints_running_downloads_as_paused() {
    let srv = TestServer::start(1_500_000, Mode::Ranges).await;
    srv.set_delay_ms(8);
    let env = Env::new().await;
    let dl = env.manager.create(req(srv.url("/file"), "sd.bin", &env)).await.unwrap();
    wait_progress(&env, &dl.id, 100_000).await;
    env.manager.shutdown(Duration::from_secs(10)).await.unwrap();
    let d = env.manager.get(&dl.id).unwrap();
    assert_eq!(d.state, DownloadState::Paused);
    assert!(d.downloaded_bytes >= 100_000);
    let _ = pattern(1);
}
