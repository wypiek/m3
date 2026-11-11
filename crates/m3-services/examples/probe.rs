//! Prints service events for a few seconds (manual integration check).
fn main() {
    tracing_subscriber_init();
    let (_svc, rx) = m3_services::Services::start();
    let (_audio, arx) = m3_services::audio::Audio::start();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(4);
    while std::time::Instant::now() < deadline {
        while let Ok(e) = rx.try_recv() {
            println!("{e:?}");
        }
        while let Ok(a) = arx.try_recv() {
            println!(
                "audio: available={} sinks={} output={:?}",
                a.available,
                a.sinks.len(),
                a.output().map(|d| (&d.description, d.volume, d.muted))
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}
fn tracing_subscriber_init() {
    tracing_subscriber::fmt().with_env_filter("info").init();
}
