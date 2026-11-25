//! Mac-only smoke test: bring the virtual display and Sunshine up, then tear them down.
//! Run it from a process that has Screen Recording permission (a Terminal window, not SSH).

#[cfg(target_os = "macos")]
#[tokio::main]
async fn main() {
    use expand_core::{macdisplay, sunshine};
    use std::time::Duration;

    let dir = std::env::temp_dir().join("expand-smoke");
    std::fs::create_dir_all(&dir).unwrap();

    // Run from inside the app bundle's folder, or the helper is not found.
    let display = macdisplay::create_virtual(1920, 1080, 60).await.expect("virtual display");
    let id = display.id;
    println!("virtual display id: {id}");

    let exe = sunshine::find_exe().expect("sunshine not found");
    let conf = sunshine::conf_path(&dir);
    std::fs::write(&conf, sunshine::conf_text(&id.to_string(), &dir)).unwrap();
    sunshine::set_credentials(&exe, &conf, "expand", "smoke-test-pass").await.expect("set credentials");

    let mut host = sunshine::Host::spawn(&exe, &conf, &sunshine::log_path(&dir)).await.expect("spawn sunshine");
    let ready = host.wait_ready(Duration::from_secs(15)).await;
    println!("sunshine ready: {ready:?}");
    let pin = host.submit_pin("expand", "smoke-test-pass", "1234", "smoke", 0).await;
    println!("submit_pin with nothing pairing: {pin:?}");

    host.kill().await;
    display.remove().await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let left = core_graphics::display::CGDisplay::active_displays().unwrap_or_default();
    println!("displays after teardown: {left:?}");
    let clean = !left.contains(&id) && !sunshine::is_port_in_use().await;
    println!("{}", if ready.is_ok() && clean { "SMOKE OK" } else { "SMOKE FAILED" });
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("host_smoke only runs on macOS");
}
