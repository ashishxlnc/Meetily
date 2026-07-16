use chrono::Utc;
use serde::{Deserialize, Serialize};
use sqlx::{Error as SqlxError, FromRow, SqlitePool};
use std::collections::HashMap;
use tracing::info;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct TagInfo {
    pub id: String,
    pub name: String,
    pub color: Option<String>,
}

pub struct TagsRepository;

impl TagsRepository {
    /// Creates a tag, or returns the existing one if a tag with this name
    /// (case-insensitively) already exists - keeps an inline "create new tag"
    /// picker idempotent instead of surfacing a UNIQUE constraint error.
    pub async fn create_tag(
        pool: &SqlitePool,
        name: &str,
        color: Option<String>,
    ) -> Result<TagInfo, SqlxError> {
        if let Some(existing) = Self::find_by_name(pool, name).await? {
            return Ok(existing);
        }

        let id = format!("tag-{}", Uuid::new_v4());
        let now = Utc::now();
        sqlx::query("INSERT INTO tags (id, name, color, created_at) VALUES (?, ?, ?, ?)")
            .bind(&id)
            .bind(name)
            .bind(&color)
            .bind(now)
            .execute(pool)
            .await?;

        info!("Created tag '{}' ({})", name, id);
        Ok(TagInfo {
            id,
            name: name.to_string(),
            color,
        })
    }

    async fn find_by_name(pool: &SqlitePool, name: &str) -> Result<Option<TagInfo>, SqlxError> {
        sqlx::query_as::<_, TagInfo>(
            "SELECT id, name, color FROM tags WHERE name = ? COLLATE NOCASE",
        )
        .bind(name)
        .fetch_optional(pool)
        .await
    }

    pub async fn list_tags(pool: &SqlitePool) -> Result<Vec<TagInfo>, SqlxError> {
        sqlx::query_as::<_, TagInfo>(
            "SELECT id, name, color FROM tags ORDER BY name COLLATE NOCASE ASC",
        )
        .fetch_all(pool)
        .await
    }

    pub async fn rename_tag(
        pool: &SqlitePool,
        tag_id: &str,
        new_name: &str,
    ) -> Result<(), SqlxError> {
        sqlx::query("UPDATE tags SET name = ? WHERE id = ?")
            .bind(new_name)
            .bind(tag_id)
            .execute(pool)
            .await?;
        Ok(())
    }

    /// Deletes a tag and its meeting assignments. Foreign key enforcement is
    /// not enabled on this SQLite connection (see delete_meeting_with_transaction
    /// in meeting.rs, which does the same manual cleanup), so ON DELETE CASCADE
    /// in the schema is not actually acted on - meeting_tags must be cleaned up
    /// explicitly or its rows orphan silently.
    pub async fn delete_tag(pool: &SqlitePool, tag_id: &str) -> Result<(), SqlxError> {
        let mut tx = pool.begin().await?;
        sqlx::query("DELETE FROM meeting_tags WHERE tag_id = ?")
            .bind(tag_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM tags WHERE id = ?")
            .bind(tag_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn assign_tag(
        pool: &SqlitePool,
        meeting_id: &str,
        tag_id: &str,
    ) -> Result<(), SqlxError> {
        let now = Utc::now();
        sqlx::query(
            "INSERT INTO meeting_tags (meeting_id, tag_id, created_at) VALUES (?, ?, ?)
             ON CONFLICT(meeting_id, tag_id) DO NOTHING",
        )
        .bind(meeting_id)
        .bind(tag_id)
        .bind(now)
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn remove_tag(
        pool: &SqlitePool,
        meeting_id: &str,
        tag_id: &str,
    ) -> Result<(), SqlxError> {
        sqlx::query("DELETE FROM meeting_tags WHERE meeting_id = ? AND tag_id = ?")
            .bind(meeting_id)
            .bind(tag_id)
            .execute(pool)
            .await?;
        Ok(())
    }

    pub async fn get_tags_for_meeting(
        pool: &SqlitePool,
        meeting_id: &str,
    ) -> Result<Vec<TagInfo>, SqlxError> {
        sqlx::query_as::<_, TagInfo>(
            "SELECT t.id, t.name, t.color
             FROM meeting_tags mt
             JOIN tags t ON t.id = mt.tag_id
             WHERE mt.meeting_id = ?
             ORDER BY t.name COLLATE NOCASE ASC",
        )
        .bind(meeting_id)
        .fetch_all(pool)
        .await
    }

    /// One query for every meeting's tags, grouped in app code. Kept separate
    /// from MeetingsRepository::get_meetings (a flat, JOIN-free read) so the
    /// sidebar's hot path doesn't reintroduce a JOIN there; meeting_tags is a
    /// small assignment table, so this stays cheap even at large meeting counts.
    pub async fn get_tags_for_all_meetings(
        pool: &SqlitePool,
    ) -> Result<HashMap<String, Vec<TagInfo>>, SqlxError> {
        let rows: Vec<(String, String, String, Option<String>)> = sqlx::query_as(
            "SELECT mt.meeting_id, t.id, t.name, t.color
             FROM meeting_tags mt
             JOIN tags t ON t.id = mt.tag_id
             ORDER BY t.name COLLATE NOCASE ASC",
        )
        .fetch_all(pool)
        .await?;

        let mut map: HashMap<String, Vec<TagInfo>> = HashMap::new();
        for (meeting_id, id, name, color) in rows {
            map.entry(meeting_id)
                .or_default()
                .push(TagInfo { id, name, color });
        }
        Ok(map)
    }
}

#[cfg(test)]
mod verify_tags {
    use super::*;
    use crate::database::manager::DatabaseManager;
    use crate::database::repositories::meeting::MeetingsRepository;

    async fn temp_db() -> (std::path::PathBuf, DatabaseManager) {
        let tmp = std::env::temp_dir().join(format!("meetily-verify-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&tmp).unwrap();
        let db_path = tmp.join("test.sqlite");
        let db_manager = DatabaseManager::new(
            db_path.to_str().unwrap(),
            tmp.join("nonexistent-legacy.db").to_str().unwrap(),
        )
        .await
        .unwrap();
        (tmp, db_manager)
    }

    async fn insert_bare_meeting(pool: &SqlitePool, title: &str) -> String {
        let meeting_id = format!("meeting-{}", Uuid::new_v4());
        let now = Utc::now();
        sqlx::query(
            "INSERT INTO meetings (id, title, created_at, updated_at) VALUES (?, ?, ?, ?)",
        )
        .bind(&meeting_id)
        .bind(title)
        .bind(now)
        .bind(now)
        .execute(pool)
        .await
        .unwrap();
        meeting_id
    }

    #[tokio::test]
    async fn create_tag_is_idempotent_by_case_insensitive_name() {
        let (tmp, db_manager) = temp_db().await;
        let pool = db_manager.pool();

        let first = TagsRepository::create_tag(pool, "Q3 Planning", Some("#ff0000".to_string()))
            .await
            .unwrap();
        let second = TagsRepository::create_tag(pool, "q3 planning", None)
            .await
            .unwrap();

        assert_eq!(first.id, second.id);
        let all_tags = TagsRepository::list_tags(pool).await.unwrap();
        assert_eq!(all_tags.len(), 1, "expected exactly one tag row, found: {:?}", all_tags);

        std::fs::remove_dir_all(&tmp).ok();
    }

    #[tokio::test]
    async fn assign_list_and_remove_tags_for_a_meeting() {
        let (tmp, db_manager) = temp_db().await;
        let pool = db_manager.pool();

        let meeting_id = insert_bare_meeting(pool, "Roadmap Sync").await;
        let tag_a = TagsRepository::create_tag(pool, "Initiative A", None).await.unwrap();
        let tag_b = TagsRepository::create_tag(pool, "Initiative B", None).await.unwrap();

        TagsRepository::assign_tag(pool, &meeting_id, &tag_a.id).await.unwrap();
        TagsRepository::assign_tag(pool, &meeting_id, &tag_b.id).await.unwrap();
        // Re-assigning the same tag should be a no-op, not an error (idempotent).
        TagsRepository::assign_tag(pool, &meeting_id, &tag_a.id).await.unwrap();

        let tags = TagsRepository::get_tags_for_meeting(pool, &meeting_id).await.unwrap();
        assert_eq!(tags.len(), 2);

        let all_map = TagsRepository::get_tags_for_all_meetings(pool).await.unwrap();
        assert_eq!(all_map.get(&meeting_id).map(|t| t.len()), Some(2));

        TagsRepository::remove_tag(pool, &meeting_id, &tag_a.id).await.unwrap();
        let tags_after_remove = TagsRepository::get_tags_for_meeting(pool, &meeting_id).await.unwrap();
        assert_eq!(tags_after_remove.len(), 1);
        assert_eq!(tags_after_remove[0].id, tag_b.id);

        std::fs::remove_dir_all(&tmp).ok();
    }

    #[tokio::test]
    async fn delete_tag_removes_its_meeting_assignments() {
        // Foreign keys are not enforced on this connection (see
        // delete_meeting_with_transaction in meeting.rs) so ON DELETE CASCADE
        // in the schema is not acted on automatically - this proves
        // TagsRepository::delete_tag cleans up meeting_tags explicitly instead
        // of leaving orphaned assignment rows behind.
        let (tmp, db_manager) = temp_db().await;
        let pool = db_manager.pool();

        let meeting_id = insert_bare_meeting(pool, "Budget Review").await;
        let tag = TagsRepository::create_tag(pool, "Finance", None).await.unwrap();
        TagsRepository::assign_tag(pool, &meeting_id, &tag.id).await.unwrap();

        TagsRepository::delete_tag(pool, &tag.id).await.unwrap();

        let orphaned: Vec<(String,)> =
            sqlx::query_as("SELECT meeting_id FROM meeting_tags WHERE tag_id = ?")
                .bind(&tag.id)
                .fetch_all(pool)
                .await
                .unwrap();
        assert!(orphaned.is_empty(), "expected no orphaned meeting_tags rows, found: {:?}", orphaned);

        let tags = TagsRepository::get_tags_for_meeting(pool, &meeting_id).await.unwrap();
        assert!(tags.is_empty());

        std::fs::remove_dir_all(&tmp).ok();
    }

    #[tokio::test]
    async fn deleting_a_meeting_removes_its_tag_assignments() {
        let (tmp, db_manager) = temp_db().await;
        let pool = db_manager.pool();

        let meeting_id = insert_bare_meeting(pool, "Sprint Retro").await;
        let tag = TagsRepository::create_tag(pool, "Engineering", None).await.unwrap();
        TagsRepository::assign_tag(pool, &meeting_id, &tag.id).await.unwrap();

        let deleted = MeetingsRepository::delete_meeting(pool, &meeting_id).await.unwrap();
        assert!(deleted);

        let orphaned: Vec<(String,)> =
            sqlx::query_as("SELECT tag_id FROM meeting_tags WHERE meeting_id = ?")
                .bind(&meeting_id)
                .fetch_all(pool)
                .await
                .unwrap();
        assert!(orphaned.is_empty(), "expected no orphaned meeting_tags rows after meeting delete, found: {:?}", orphaned);

        // The tag itself should survive (only the assignment is removed).
        let all_tags = TagsRepository::list_tags(pool).await.unwrap();
        assert!(all_tags.iter().any(|t| t.id == tag.id));

        std::fs::remove_dir_all(&tmp).ok();
    }
}
