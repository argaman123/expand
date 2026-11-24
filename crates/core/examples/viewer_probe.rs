//! Dev probe (Windows): runs the app's own viewer engine (`RealViewer`, with a stand-in display
//! window) against a host, and tells how the start ends: an error Moonlight put up (and after how
//! long), the fallback, or Moonlight closing.
//!
//! `viewer_probe <Moonlight.exe> <host> <seconds>`
//!
//! Run it from a portable copy of Moonlight (a `portable.dat` beside it, and that folder as the
//! working directory), so the installed Moonlight's settings are never touched.

#[cfg(windows)]
fn main() {
    use sidescreen_core::config::{Placement, StreamSettings};
    use sidescreen_core::session::engines::{RealViewer, ViewerHost};
    use sidescreen_core::session::ViewerEngine;
    use sidescreen_core::share::geometry::Area;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    struct StandIn(Instant);
    impl ViewerHost for StandIn {
        fn attach(&self, window: isize) -> Result<(), String> {
            println!("{:>6} ms  attach {window:#x} (the stand-in refuses)", self.0.elapsed().as_millis());
            Err("no display window here".into())
        }
        fn detach(&self) {}
        fn fallback(&self) {
            println!("{:>6} ms  fallback", self.0.elapsed().as_millis());
        }
        fn end(&self) {}
        fn area(&self) -> Option<Area> {
            sidescreen_core::winwindow::monitors().first().copied()
        }
    }

    let args: Vec<String> = std::env::args().collect();
    let (exe, host, secs) = (&args[1], &args[2], args[3].parse::<u64>().unwrap());
    sidescreen_core::sharelog::enable();
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let started = Instant::now();
        let mut viewer = RealViewer::new(Placement::default(), None)
            .unwrap()
            .with_exe(exe.into())
            .with_host(Arc::new(StandIn(started)));
        viewer.start(host, &StreamSettings::default()).await.unwrap();
        // Who has the front, as it changes.
        std::thread::spawn(move || {
            let mut last = 0;
            loop {
                let front = sidescreen_core::winwindow::foreground_window();
                if front != last {
                    let pid = sidescreen_core::winwindow::process_of_window(front);
                    println!("{:>6} ms  in front: {front:#x} of pid {pid}", started.elapsed().as_millis());
                    last = front;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        });
        let ended = tokio::time::timeout(Duration::from_secs(secs), viewer.closed()).await.is_ok();
        let said = viewer.last_words();
        println!(
            "{:>6} ms  {}; Moonlight said: {:?}",
            started.elapsed().as_millis(),
            if ended { "the start ended" } else { "still going" },
            said
        );
        viewer.stop().await;
    });
}

#[cfg(not(windows))]
fn main() {}
