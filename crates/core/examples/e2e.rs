//! Headless end-to-end check, without the app window.
//!   Mac:  e2e host                (run from a Terminal window so it has Screen Recording permission)
//!   PC:   e2e client <mac-ip> [seconds-to-stay-connected]
//! The host accepts pairing without asking, so only run it for testing.

use sidescreen_core::config::Config;
use sidescreen_core::session::State;

fn load() -> (Config, std::path::PathBuf) {
    let path = Config::default_path();
    (Config::load(&path).expect("settings file"), path)
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("client") => client(&args[2], args.get(3).and_then(|s| s.parse().ok()).unwrap_or(20)).await,
        #[cfg(target_os = "macos")]
        Some("host") => host().await,
        _ => eprintln!("usage: e2e host | e2e client <mac-ip> [seconds]"),
    }
}

async fn client(ip: &str, seconds: u64) {
    use sidescreen_core::link::CONTROL_PORT;
    use sidescreen_core::session::{client::run, engines::RealViewer};
    use tokio_util::sync::CancellationToken;

    let (mut cfg, path) = load();
    let addr: std::net::SocketAddr = format!("{ip}:{CONTROL_PORT}").parse().expect("address");
    let mut viewer = RealViewer::new(cfg.mac_side).expect("Moonlight");
    let (tx, mut rx) = tokio::sync::watch::channel(State::Disconnected);
    let cancel = CancellationToken::new();

    let printer = tokio::spawn(async move {
        while rx.changed().await.is_ok() {
            println!("state: {:?}", *rx.borrow());
        }
    });
    let timer = cancel.clone();
    tokio::spawn(async move {
        let mut watch = timer.clone();
        let _ = &mut watch;
        tokio::time::sleep(std::time::Duration::from_secs(seconds)).await;
        println!("time is up: disconnecting");
        timer.cancel();
    });

    if let Some(peer) = run(vec![addr], &cfg, &mut viewer, &tx, cancel, &|_| {}).await {
        println!("saving peer (sunshine_paired = {})", peer.sunshine_paired);
        cfg.peer = Some(peer);
        cfg.save(&path).expect("save settings");
    }
    drop(tx);
    let _ = printer.await;
}

#[cfg(target_os = "macos")]
async fn host() {
    use async_trait::async_trait;
    use sidescreen_core::link::{server::Server, TrustPrompt, CONTROL_PORT};
    use sidescreen_core::session::{engines::RealHostEngines, host::serve};

    struct AcceptAll;
    #[async_trait]
    impl TrustPrompt for AcceptAll {
        async fn ask(&self, name: &str, _id: &str) -> bool {
            println!("auto-accepting pairing from {name}");
            true
        }
    }

    let (mut cfg, path) = load();
    cfg.save(&path).expect("save settings");
    let data_dir = path.parent().unwrap().join("sunshine");
    let server = Server::bind(format!("0.0.0.0:{CONTROL_PORT}").parse().unwrap()).await.expect("bind");
    println!("listening on {}", server.local_addr());
    loop {
        match server.accept(|| cfg.peer.clone(), &AcceptAll).await {
            Ok((conn, new_peer)) => {
                if let Some(peer) = new_peer {
                    cfg.peer = Some(peer);
                    cfg.save(&path).expect("save settings");
                }
                println!("client connected");
                let mut engines =
                    RealHostEngines::new(data_dir.clone(), cfg.sunshine_user.clone(), cfg.sunshine_pass.clone());
                let (tx, _rx) = tokio::sync::watch::channel(State::Disconnected);
                serve(conn, &mut engines, &tx, &|_| {}).await;
                println!("session ended");
            }
            Err(e) => println!("rejected a connection: {e}"),
        }
    }
}
