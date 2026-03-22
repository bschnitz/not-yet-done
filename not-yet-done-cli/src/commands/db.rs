use tusks::tusks;

#[tusks()]
#[command(about = "Database operations")]
pub mod cli {
    pub use crate::cli as parent_;

    /// Synchronize the database schema with the current entity definitions
    pub fn sync() -> u8 {
        println!("✓ Schema synchronized.");
        0
    }
}
