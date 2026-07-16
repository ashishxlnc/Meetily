use crate::api::{MeetingDetails, MeetingTranscript};
use crate::database::models::{DateTimeUtc, MeetingModel, Transcript};
use chrono::Utc;
use sqlx::{Connection, Error as SqlxError, FromRow, SqliteConnection, SqlitePool};
use tracing::{error, info};

pub struct MeetingsRepository;

/// Meeting row for the list view. `duration_seconds` is denormalized onto
/// `meetings` at write time (see `TranscriptsRepository::save_transcript`),
/// so this is a flat, indexed read with no JOIN/aggregation against the
/// (much larger) transcripts table.
#[derive(Debug, Clone, FromRow)]
pub struct MeetingListRow {
    pub id: String,
    pub title: String,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
    pub folder_path: Option<String>,
    pub duration_seconds: Option<f64>,
}

impl MeetingsRepository {
    pub async fn get_meetings(pool: &SqlitePool) -> Result<Vec<MeetingListRow>, sqlx::Error> {
        let meetings = sqlx::query_as::<_, MeetingListRow>(
            "SELECT id, title, created_at, updated_at, folder_path, duration_seconds
             FROM meetings
             ORDER BY created_at DESC",
        )
        .fetch_all(pool)
        .await?;
        Ok(meetings)
    }

    pub async fn delete_meeting(pool: &SqlitePool, meeting_id: &str) -> Result<bool, SqlxError> {
        if meeting_id.trim().is_empty() {
            return Err(SqlxError::Protocol(
                "meeting_id cannot be empty".to_string(),
            ));
        }

        let mut conn = pool.acquire().await?;
        let mut transaction = conn.begin().await?;

        match delete_meeting_with_transaction(&mut transaction, meeting_id).await {
            Ok(success) => {
                if success {
                    transaction.commit().await?;
                    info!(
                        "Successfully deleted meeting {} and all associated data",
                        meeting_id
                    );
                    Ok(true)
                } else {
                    transaction.rollback().await?;
                    Ok(false)
                }
            }
            Err(e) => {
                let _ = transaction.rollback().await;
                error!("Failed to delete meeting {}: {}", meeting_id, e);
                Err(e)
            }
        }
    }

    pub async fn get_meeting(
        pool: &SqlitePool,
        meeting_id: &str,
    ) -> Result<Option<MeetingDetails>, SqlxError> {
        if meeting_id.trim().is_empty() {
            return Err(SqlxError::Protocol(
                "meeting_id cannot be empty".to_string(),
            ));
        }

        let mut conn = pool.acquire().await?;
        let mut transaction = conn.begin().await?;

        // Get meeting details
        let meeting: Option<MeetingModel> =
            sqlx::query_as("SELECT id, title, created_at, updated_at, folder_path FROM meetings WHERE id = ?")
                .bind(meeting_id)
                .fetch_optional(&mut *transaction)
                .await?;

        if meeting.is_none() {
            transaction.rollback().await?;
            return Err(SqlxError::RowNotFound);
        }

        if let Some(meeting) = meeting {
            // Get all transcripts for this meeting
            let transcripts =
                sqlx::query_as::<_, Transcript>("SELECT * FROM transcripts WHERE meeting_id = ?")
                    .bind(meeting_id)
                    .fetch_all(&mut *transaction)
                    .await?;

            transaction.commit().await?;

            // Convert Transcript to MeetingTranscript
            let meeting_transcripts = transcripts
                .into_iter()
                .map(|t| MeetingTranscript {
                    id: t.id,
                    text: t.transcript,
                    timestamp: t.timestamp,
                    audio_start_time: t.audio_start_time,
                    audio_end_time: t.audio_end_time,
                    duration: t.duration,
                })
                .collect::<Vec<_>>();

            Ok(Some(MeetingDetails {
                id: meeting.id,
                title: meeting.title,
                created_at: meeting.created_at.0.to_rfc3339(),
                updated_at: meeting.updated_at.0.to_rfc3339(),
                transcripts: meeting_transcripts,
            }))
        } else {
            transaction.rollback().await?;
            Ok(None)
        }
    }

    /// Get meeting metadata without transcripts (for pagination)
    pub async fn get_meeting_metadata(
        pool: &SqlitePool,
        meeting_id: &str,
    ) -> Result<Option<MeetingModel>, SqlxError> {
        if meeting_id.trim().is_empty() {
            return Err(SqlxError::Protocol(
                "meeting_id cannot be empty".to_string(),
            ));
        }

        let meeting: Option<MeetingModel> =
            sqlx::query_as("SELECT id, title, created_at, updated_at, folder_path FROM meetings WHERE id = ?")
                .bind(meeting_id)
                .fetch_optional(pool)
                .await?;

        Ok(meeting)
    }

    /// Get meeting transcripts with pagination support
    pub async fn get_meeting_transcripts_paginated(
        pool: &SqlitePool,
        meeting_id: &str,
        limit: i64,
        offset: i64,
    ) -> Result<(Vec<Transcript>, i64), SqlxError> {
        if meeting_id.trim().is_empty() {
            return Err(SqlxError::Protocol(
                "meeting_id cannot be empty".to_string(),
            ));
        }

        // Get total count of transcripts for this meeting
        let total: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM transcripts WHERE meeting_id = ?"
        )
        .bind(meeting_id)
        .fetch_one(pool)
        .await?;

        // Get paginated transcripts ordered by audio_start_time
        let transcripts = sqlx::query_as::<_, Transcript>(
            "SELECT * FROM transcripts
             WHERE meeting_id = ?
             ORDER BY audio_start_time ASC
             LIMIT ? OFFSET ?"
        )
        .bind(meeting_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await?;

        Ok((transcripts, total.0))
    }

    pub async fn update_meeting_title(
        pool: &SqlitePool,
        meeting_id: &str,
        new_title: &str,
    ) -> Result<bool, SqlxError> {
        if meeting_id.trim().is_empty() {
            return Err(SqlxError::Protocol(
                "meeting_id cannot be empty".to_string(),
            ));
        }

        let mut conn = pool.acquire().await?;
        let mut transaction = conn.begin().await?;

        let now = Utc::now().naive_utc();

        let rows_affected =
            sqlx::query("UPDATE meetings SET title = ?, updated_at = ? WHERE id = ?")
                .bind(new_title)
                .bind(now)
                .bind(meeting_id)
                .execute(&mut *transaction)
                .await?;
        if rows_affected.rows_affected() == 0 {
            transaction.rollback().await?;
            return Ok(false);
        }
        transaction.commit().await?;
        Ok(true)
    }

    pub async fn update_meeting_name(
        pool: &SqlitePool,
        meeting_id: &str,
        new_title: &str,
    ) -> Result<bool, SqlxError> {
        let mut transaction = pool.begin().await?;
        let now = Utc::now();

        // Update meetings table
        let meeting_update =
            sqlx::query("UPDATE meetings SET title = ?, updated_at = ? WHERE id = ?")
                .bind(new_title)
                .bind(now)
                .bind(meeting_id)
                .execute(&mut *transaction)
                .await?;

        if meeting_update.rows_affected() == 0 {
            transaction.rollback().await?;
            return Ok(false); // Meeting not found
        }

        // Update transcript_chunks table
        sqlx::query("UPDATE transcript_chunks SET meeting_name = ? WHERE meeting_id = ?")
            .bind(new_title)
            .bind(meeting_id)
            .execute(&mut *transaction)
            .await?;

        transaction.commit().await?;
        Ok(true)
    }
}

async fn delete_meeting_with_transaction(
    transaction: &mut SqliteConnection,
    meeting_id: &str,
) -> Result<bool, SqlxError> {
    // Check if meeting exists
    let meeting_exists: Option<(i64,)> = sqlx::query_as("SELECT 1 FROM meetings WHERE id = ?")
        .bind(meeting_id)
        .fetch_optional(&mut *transaction)
        .await?;

    if meeting_exists.is_none() {
        error!("Meeting {} not found for deletion", meeting_id);
        return Ok(false);
    }

    // Delete from related tables in proper order
    // 1. Delete from transcript_chunks
    sqlx::query("DELETE FROM transcript_chunks WHERE meeting_id = ?")
        .bind(meeting_id)
        .execute(&mut *transaction)
        .await?;

    // 2. Delete from summary_processes
    sqlx::query("DELETE FROM summary_processes WHERE meeting_id = ?")
        .bind(meeting_id)
        .execute(&mut *transaction)
        .await?;

    // 3. Delete from transcripts
    sqlx::query("DELETE FROM transcripts WHERE meeting_id = ?")
        .bind(meeting_id)
        .execute(&mut *transaction)
        .await?;

    // 4. Delete from meeting_tags (tag assignments) - foreign keys are not
    // enforced on this connection, so this must be explicit, same as above.
    sqlx::query("DELETE FROM meeting_tags WHERE meeting_id = ?")
        .bind(meeting_id)
        .execute(&mut *transaction)
        .await?;

    // 5. Finally, delete the meeting
    let result = sqlx::query("DELETE FROM meetings WHERE id = ?")
        .bind(meeting_id)
        .execute(&mut *transaction)
        .await?;

    Ok(result.rows_affected() > 0)
}

#[cfg(test)]
mod verify_meeting_list_perf {
    use super::*;
    use crate::api::TranscriptSegment;
    use crate::database::manager::DatabaseManager;
    use crate::database::repositories::transcript::TranscriptsRepository;
    use uuid::Uuid;

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

    fn segment(id: &str, start: f64, end: f64) -> TranscriptSegment {
        TranscriptSegment {
            id: id.to_string(),
            text: format!("segment {id}"),
            timestamp: "00:00:00".to_string(),
            audio_start_time: Some(start),
            audio_end_time: Some(end),
            duration: Some(end - start),
        }
    }

    #[tokio::test]
    async fn indexes_exist_after_migration() {
        let (tmp, db_manager) = temp_db().await;
        let pool = db_manager.pool();

        let transcripts_indexes: Vec<(String,)> =
            sqlx::query_as("SELECT name FROM sqlite_master WHERE type = 'index' AND tbl_name = 'transcripts'")
                .fetch_all(pool)
                .await
                .unwrap();
        assert!(
            transcripts_indexes.iter().any(|(name,)| name == "idx_transcripts_meeting_id"),
            "expected idx_transcripts_meeting_id to exist, found: {:?}",
            transcripts_indexes
        );

        let meetings_indexes: Vec<(String,)> =
            sqlx::query_as("SELECT name FROM sqlite_master WHERE type = 'index' AND tbl_name = 'meetings'")
                .fetch_all(pool)
                .await
                .unwrap();
        assert!(
            meetings_indexes.iter().any(|(name,)| name == "idx_meetings_created_at"),
            "expected idx_meetings_created_at to exist, found: {:?}",
            meetings_indexes
        );

        std::fs::remove_dir_all(&tmp).ok();
    }

    #[tokio::test]
    async fn get_meetings_returns_denormalized_duration_with_no_join() {
        let (tmp, db_manager) = temp_db().await;
        let pool = db_manager.pool();

        // Segments arrive out of order and with gaps (VAD-filtered speech spans);
        // duration should span from the earliest start to the latest end.
        let segments = vec![
            segment("a", 5.0, 8.0),
            segment("b", 0.0, 3.0),
            segment("c", 20.0, 27.5),
        ];
        let meeting_id = TranscriptsRepository::save_transcript(pool, "Perf Test Meeting", &segments, None)
            .await
            .unwrap();

        let rows = MeetingsRepository::get_meetings(pool).await.unwrap();
        let row = rows.iter().find(|r| r.id == meeting_id).unwrap();
        assert_eq!(row.duration_seconds, Some(27.5));

        // No transcripts row should be needed for this to work - drop them and confirm
        // the list row is unaffected, proving the value truly is denormalized and the
        // query has no live JOIN dependency on the transcripts table.
        sqlx::query("DELETE FROM transcripts WHERE meeting_id = ?")
            .bind(&meeting_id)
            .execute(pool)
            .await
            .unwrap();
        let rows_after_delete = MeetingsRepository::get_meetings(pool).await.unwrap();
        let row_after_delete = rows_after_delete.iter().find(|r| r.id == meeting_id).unwrap();
        assert_eq!(row_after_delete.duration_seconds, Some(27.5));

        std::fs::remove_dir_all(&tmp).ok();
    }

    #[tokio::test]
    async fn get_meetings_duration_is_none_with_no_segments() {
        let (tmp, db_manager) = temp_db().await;
        let pool = db_manager.pool();

        let meeting_id = TranscriptsRepository::save_transcript(pool, "Empty Meeting", &[], None)
            .await
            .unwrap();

        let rows = MeetingsRepository::get_meetings(pool).await.unwrap();
        let row = rows.iter().find(|r| r.id == meeting_id).unwrap();
        assert_eq!(row.duration_seconds, None);

        std::fs::remove_dir_all(&tmp).ok();
    }

    #[tokio::test]
    async fn migration_backfill_computes_duration_for_pre_existing_rows() {
        let (tmp, db_manager) = temp_db().await;
        let pool = db_manager.pool();

        // Simulate a meeting that existed *before* this migration: inserted with
        // duration_seconds left NULL, same as every pre-migration row would be.
        let meeting_id = format!("meeting-{}", Uuid::new_v4());
        let now = Utc::now();
        sqlx::query(
            "INSERT INTO meetings (id, title, created_at, updated_at, folder_path, duration_seconds)
             VALUES (?, ?, ?, ?, NULL, NULL)",
        )
        .bind(&meeting_id)
        .bind("Pre-existing Meeting")
        .bind(now)
        .bind(now)
        .execute(pool)
        .await
        .unwrap();

        for (start, end) in [(0.0, 4.0), (10.0, 15.5)] {
            sqlx::query(
                "INSERT INTO transcripts (id, meeting_id, transcript, timestamp, audio_start_time, audio_end_time, duration)
                 VALUES (?, ?, 'x', '00:00:00', ?, ?, ?)",
            )
            .bind(format!("transcript-{}", Uuid::new_v4()))
            .bind(&meeting_id)
            .bind(start)
            .bind(end)
            .bind(end - start)
            .execute(pool)
            .await
            .unwrap();
        }

        // Re-run exactly the backfill statement from the migration - proves it
        // correctly derives duration_seconds for rows that predate this change.
        sqlx::query(
            "UPDATE meetings
             SET duration_seconds = (
                 SELECT MAX(t.audio_end_time) - MIN(t.audio_start_time)
                 FROM transcripts t
                 WHERE t.meeting_id = meetings.id
             )
             WHERE EXISTS (SELECT 1 FROM transcripts t WHERE t.meeting_id = meetings.id)",
        )
        .execute(pool)
        .await
        .unwrap();

        let rows = MeetingsRepository::get_meetings(pool).await.unwrap();
        let row = rows.iter().find(|r| r.id == meeting_id).unwrap();
        assert_eq!(row.duration_seconds, Some(15.5));

        std::fs::remove_dir_all(&tmp).ok();
    }
}
