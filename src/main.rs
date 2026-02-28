mod app;
mod audio;
mod net;
mod recording;
mod video;

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    log::info!("HyperZoom starting");

    let runtime = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");

    let native_options = eframe::NativeOptions::default();
    eframe::run_native(
        "HyperZoom",
        native_options,
        Box::new(|_cc| Ok(Box::new(app::HyperZoomApp::new(runtime)))),
    )
    .expect("eframe failed");
}
