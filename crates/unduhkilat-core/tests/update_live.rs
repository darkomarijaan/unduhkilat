//! Real-network check of the updater against the published GitHub releases. `#[ignore]`d: run with
//! `cargo test -p unduhkilat-core --test update_live -- --ignored --nocapture`.

use unduhkilat_core::config::EngineConfig;
use unduhkilat_core::engine::{http, proxy::ProxyConfig};
use unduhkilat_core::update;

#[tokio::test]
#[ignore]
async fn the_newest_release_downloads_and_verifies() {
    let client = http::build_client(&EngineConfig::default(), &ProxyConfig::default()).unwrap();
    let info = update::check(&client, "0.0.1").await.unwrap().expect("a signed release exists");
    println!("rilis {}: sumber zip {:?}", info.version, info.zip_sources);
    let started = std::time::Instant::now();
    let zip = update::download(&client, &info).await.unwrap();
    println!("{} byte diunduh dan tanda tangan valid dalam {:.1?}", zip.len(), started.elapsed());
}
