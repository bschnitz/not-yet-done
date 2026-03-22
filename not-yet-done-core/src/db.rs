use sea_orm::{Database, DatabaseConnection, DbErr};

pub async fn connect(db_url: &str, sync_schema: bool) -> Result<DatabaseConnection, DbErr> {
    let db = Database::connect(db_url).await?;

    if sync_schema {
        db.get_schema_registry("not_yet_done_core::entity::*")
            .sync(&db)
            .await?;
    }

    Ok(db)
}
