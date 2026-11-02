//! Reproducible metadata benchmark on a disposable, flat-to-shallow synthetic tree.
use spacemosaic::scan::{self, Options, Progress};
use std::{
    fs,
    sync::{Mutex, atomic::AtomicBool},
    time::{SystemTime, UNIX_EPOCH},
};
struct Fixture(std::path::PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn main() -> std::io::Result<()> {
    let count = std::env::args()
        .nth(1)
        .and_then(|arg| arg.parse::<usize>().ok())
        .unwrap_or(100_000);
    let root = std::env::temp_dir().join(format!(
        "spacemosaic-benchmark-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    fs::create_dir(&root)?;
    let fixture = Fixture(root);
    for directory in 0..100 {
        fs::create_dir(fixture.0.join(directory.to_string()))?;
    }
    for index in 0..count {
        let file = fs::File::create(
            fixture
                .0
                .join((index % 100).to_string())
                .join(format!("file-{index}.bin")),
        )?;
        file.set_len((index % 8192) as u64)?;
    }
    let result = scan::scan(
        &fixture.0,
        Options {
            memory_mode: std::env::args()
                .nth(2)
                .and_then(|v| v.parse::<u32>().ok())
                .filter(|v| (16..=4096).contains(v))
                .map(|mib| spacemosaic::memory::BudgetMode::Manual { mib })
                .unwrap_or_default(),
            ..Default::default()
        },
        &AtomicBool::new(false),
        &Mutex::new(Progress::default()),
    );
    println!(
        "{}",
        serde_json::json!({"fixture_files":count,"objects":result.nodes.len(),"scan_millis":result.elapsed_millis,
        "known_logical_bytes":result.nodes[0].logical.known,"errors":result.errors,"limited":result.limited,"limit_reason":result.limit_reason,"memory":result.memory,"platform":std::env::consts::OS,"architecture":std::env::consts::ARCH})
    );
    Ok(())
}
