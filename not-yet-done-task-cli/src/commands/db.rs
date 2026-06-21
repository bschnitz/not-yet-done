use tusks::tusks;

#[tusks()]
#[command(about = "Database operations")]
pub mod cli {
    pub use crate::cli as parent_;

    /// Synchronize the database schema with the current entity definitions.
    ///
    /// The actual schema sync runs at connection time (`main` opens the DB with
    /// schema-sync enabled when it sees `db sync` in the args); this command
    /// body just reports success once that has happened.
    #[command(about = "Create or upgrade the database schema")]
    pub fn sync() -> u8 {
        println!("✓ Schema synchronized.");
        0
    }
}
