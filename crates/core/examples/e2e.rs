//! Headless end-to-end check, without the app window.
//!   Mac:  e2e host                (run from a Terminal window so it has Screen Recording permission)
//!   PC:   e2e client <mac-ip> [seconds-to-stay-connected]
//! The host pairs with a fixed code, so only run it for testing.
//! It reads and writes the settings file named by `EXPAND_CONFIG`, and refuses to run without
//! one, so it never changes the app's own settings.

use expand_core::config::Config;
use expand_core::session::State;

/// The pairing code both sides use in this test tool.
const CODE: &str = "000000";

fn load() -> (Config, std::path::PathBuf) {
    let path = Config::default_path();
    (Config::load(&path).expect("settings file"), path)
}

#[tokio::main]
async fn main() {
    if std::env::var_os("EXPAND_CONFIG").filter(|p| !p.is_empty()).is_none() {
        eprintln!("e2e: set EXPAND_CONFIG to a settings file for this test, for example in a temporary folder.");
        eprintln!("Without it, e2e would change the app's own settings and pairings.");
        std::process::exit(2);
    }
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("client") => client(&args[2], args.get(3).and_then(|s| s.parse().ok()).unwrap_or(20)).await,
        #[cfg(target_os = "macos")]
        Some("host") => host().await,
        _ => eprintln!("usage: e2e host | e2e client <mac-ip> [seconds]"),
    }
}

async fn client(ip: &str, seconds: u64) {
    use expand_core::link::CONTROL_PORT;
    use expand_core::session::{client::run, engines::RealViewer};
    use tokio_util::sync::CancellationToken;

    let (mut cfg, path) = load();
    let addr: std::net::SocketAddr = format!("{ip}:{CONTROL_PORT}").parse().expect("address");
    let mut viewer = RealViewer::new(cfg.current_peer().map(|p| p.placement).unwrap_or_default(), cfg.current_peer().and_then(|p| p.stream.monitor)).expect("Moonlight");
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

    if let Some(peer) = run(vec![addr], &cfg, &mut viewer, &tx, cancel, &|_| {}, Some(CODE)).await {
        println!("saving peer (sunshine_paired = {})", peer.sunshine_paired);
        cfg.current = Some(cfg.upsert_peer(peer));
        cfg.save(&path).expect("save settings");
    }
    drop(tx);
    let _ = printer.await;
}

#[cfg(target_os = "macos")]
async fn host() {
    use expand_core::link::{server::Server, PairGate, CONTROL_PORT};
    use expand_core::session::{engines::RealHostEngines, host::serve};

    struct AcceptAll;
    impl PairGate for AcceptAll {
        fn code(&self) -> Option<String> {
            Some(CODE.to_string())
        }

        fn conclude(&self, _code: &str, proved: bool) -> bool {
            println!("pairing: {}", if proved { "accepted" } else { "wrong code" });
            proved
        }
    }

    let (mut cfg, path) = load();
    cfg.save(&path).expect("save settings");
    let data_dir = path.parent().unwrap().join("sunshine");
    let mut server = Server::bind(format!("0.0.0.0:{CONTROL_PORT}").parse().unwrap()).await.expect("bind");
    let gate: std::sync::Arc<dyn PairGate> = std::sync::Arc::new(AcceptAll);
    println!("listening on {}", server.local_addr());
    loop {
        let known = cfg.clone();
        let accepted = server
            .accept(move |id| known.peer(id).filter(|p| p.allowed).cloned(), gate.clone(), (&cfg.device_name, &cfg.device_id))
            .await;
        match accepted {
            Ok((conn, new_peer, _)) => {
                if let Some(peer) = new_peer {
                    cfg.current = Some(cfg.upsert_peer(peer));
                    cfg.save(&path).expect("save settings");
                }
                println!("client connected");
                let toward_pc = cfg.current_peer().map(|p| p.placement.side.opposite()).unwrap_or(expand_core::side::Side::Right);
                let mut engines = RealHostEngines::new(
                    data_dir.clone(),
                    cfg.sunshine_user.clone(),
                    cfg.sunshine_pass.clone(),
                    move || toward_pc,
                );
                let (tx, _rx) = tokio::sync::watch::channel(State::Disconnected);
                serve(conn, &mut engines, &tx, &|_| {}).await;
                println!("session ended");
            }
            Err(e) => println!("rejected a connection: {e}"),
        }
    }
}
