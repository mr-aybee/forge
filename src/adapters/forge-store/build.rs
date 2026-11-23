//! Rebuild when a migration changes: `sqlx::migrate!` embeds the SQL files at
//! compile time, and on stable Rust it cannot tell Cargo to watch them.
fn main() {
    println!("cargo:rerun-if-changed=migrations");
}
