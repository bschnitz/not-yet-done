//! Automatic trigger bound to a view script.
//!
//! View scripts themselves live on disk (`ScriptStore`, i.e.
//! `<XDG_DATA_HOME>/not_yet_done/scripts/<adapter>/<path>/<name>`) and are
//! normally run by hand — from the script menu or through a key chord held
//! in [`query_shortcut`](super::query_shortcut). This table holds the other
//! way to start them: which *event* should run the script named `name` on a
//! particular view scope, without anyone pressing a key.
//!
//! `scope` uses the same key as the chord binding, so a script's shortcut
//! and its hook are found by one and the same lookup path — see the
//! [`query_shortcut`](super::query_shortcut) entity docs for the shape of a
//! scope string.
//!
//! `hook` names the event. Today the only one is `reload`: run the script
//! right after the view's own load finished. Rows with an unknown hook name
//! are ignored by the frontend rather than being an error, so a database
//! written by a newer binary stays readable by an older one.
//!
//! There is no FK to the script body — it lives on the filesystem. A hook
//! whose script was deleted behind the app's back is silently skipped.
use sea_orm::Set;
use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "script_hook")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub scope: String,
    pub name: String,
    /// The event that triggers the script. `reload` today.
    pub hook: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {
    fn new() -> Self {
        Self {
            id: Set(Uuid::new_v4()),
            ..ActiveModelTrait::default()
        }
    }
}
