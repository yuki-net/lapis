use std::{sync::Arc, time::Duration};

fn main() {
    eprintln!("probe-start pid={}", std::process::id());
    subsecond::register_handler(Arc::new(|| {
        eprintln!("patch-applied pid={}", std::process::id());
    }));
    dioxus_devtools::connect_subsecond();
    loop {
        let value = subsecond::HotFn::current(value).call(());
        eprintln!("probe-value pid={} value={value}", std::process::id());
        std::thread::sleep(Duration::from_millis(500));
    }
}

#[inline(never)]
fn value() -> u64 {
    let bytes = std::hint::black_box([0u8; 16384]);
    std::hint::black_box(&bytes);
    105
}
