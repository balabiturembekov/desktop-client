use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow, Clone)]
#[allow(dead_code)]
pub struct Screenshot {
    pub id: i64,
    pub time_slot_id: i64,
    pub file_path: String,
    pub taken_at: String,
    pub synced: i64,
}
