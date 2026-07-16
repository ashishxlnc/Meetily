use crate::api::{TranscriptSearchResult, TranscriptSegment};
use crate::audio::recording_saver::MeetingMetadata;
use chrono::{DateTime, Utc};
use sqlx::{Connection, Error as SqlxError, SqlitePool};
use tracing::{error, info};
use uuid::Uuid;

pub struct TranscriptsRepository;

impl TranscriptsRepository {
    /// Reads the true wall-clock recording-start time from the meeting folder's
    /// metadata.json (written by `recording_saver::initialize_meeting_folder` when
    /// the recording begins). Falls back to `Utc::now()` if the folder/file is
    /// missing or unparseable (e.g. crash-recovery saves without a folder).
    fn resolve_meeting_start_time(folder_path: &Option<String>) -> DateTime<Utc> {
        folder_path
            .as_ref()
            .and_then(|folder| std::fs::read_to_string(std::path::Path::new(folder).join("metadata.json")).ok())
            .and_then(|contents| serde_json::from_str::<MeetingMetadata>(&contents).ok())
            .and_then(|metadata| DateTime::parse_from_rfc3339(&metadata.created_at).ok())
            .map(|parsed| parsed.with_timezone(&Utc))
            .unwrap_or_else(Utc::now)
    }

    /// Computes total recording duration from a set of transcript segments'
    /// audio-relative offsets, mirroring the guard previously applied at read
    /// time in `api_get_meetings` (end must be strictly after start).
    pub(crate) fn compute_duration_seconds(transcripts: &[TranscriptSegment]) -> Option<f64> {
        let start = transcripts
            .iter()
            .filter_map(|s| s.audio_start_time)
            .fold(f64::INFINITY, f64::min);
        let end = transcripts
            .iter()
            .filter_map(|s| s.audio_end_time)
            .fold(f64::NEG_INFINITY, f64::max);
        (end > start).then_some(end - start)
    }

    /// Saves a new meeting and its associated transcript segments.
    /// This function uses a transaction to ensure that either both the meeting
    /// and all its transcripts are saved, or none of them are.
    pub async fn save_transcript(
        pool: &SqlitePool,
        meeting_title: &str,
        transcripts: &[TranscriptSegment],
        folder_path: Option<String>,
    ) -> Result<String, SqlxError> {
        let meeting_id = format!("meeting-{}", Uuid::new_v4());

        let mut conn = pool.acquire().await?;
        let mut transaction = conn.begin().await?;

        let started_at = Self::resolve_meeting_start_time(&folder_path);
        let saved_at = Utc::now();
        let duration_seconds = Self::compute_duration_seconds(transcripts);

        // 1. Create the new meeting
        let result = sqlx::query(
            "INSERT INTO meetings (id, title, created_at, updated_at, folder_path, duration_seconds) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&meeting_id)
        .bind(meeting_title)
        .bind(started_at)
        .bind(saved_at)
        .bind(&folder_path)
        .bind(duration_seconds)
        .execute(&mut *transaction)
        .await;

        if let Err(e) = result {
            error!("Failed to create meeting '{}': {}", meeting_title, e);
            transaction.rollback().await?;
            return Err(e);
        }

        info!("Successfully created meeting with id: {}", meeting_id);

        // 2. Save each transcript segment with audio timing fields
        for segment in transcripts {
            let transcript_id = format!("transcript-{}", Uuid::new_v4());
            let result = sqlx::query(
                "INSERT INTO transcripts (id, meeting_id, transcript, timestamp, audio_start_time, audio_end_time, duration)
                 VALUES (?, ?, ?, ?, ?, ?, ?)"
            )
            .bind(&transcript_id)
            .bind(&meeting_id)
            .bind(&segment.text)
            .bind(&segment.timestamp)
            .bind(segment.audio_start_time)
            .bind(segment.audio_end_time)
            .bind(segment.duration)
            .execute(&mut *transaction)
            .await;

            if let Err(e) = result {
                error!(
                    "Failed to save transcript segment for meeting {}: {}",
                    meeting_id, e
                );
                transaction.rollback().await?;
                return Err(e);
            }
        }

        info!(
            "Successfully saved {} transcript segments for meeting {}",
            transcripts.len(),
            meeting_id
        );

        // Commit the transaction
        transaction.commit().await?;

        Ok(meeting_id)
    }

    /// Searches for a query string within the transcripts.
    /// It returns a list of matching transcripts with context.
    pub async fn search_transcripts(
        pool: &SqlitePool,
        query: &str,
    ) -> Result<Vec<TranscriptSearchResult>, SqlxError> {
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }

        let search_query = format!("%{}%", query.to_lowercase());

        let rows = sqlx::query_as::<_, (String, String, String, String)>(
            "SELECT m.id, m.title, t.transcript, t.timestamp
             FROM meetings m
             JOIN transcripts t ON m.id = t.meeting_id
             WHERE LOWER(t.transcript) LIKE ?",
        )
        .bind(&search_query)
        .fetch_all(pool)
        .await?;

        let results = rows
            .into_iter()
            .map(|(id, title, transcript, timestamp)| {
                let match_context = Self::get_match_context(&transcript, query);
                TranscriptSearchResult {
                    id,
                    title,
                    match_context,
                    timestamp,
                }
            })
            .collect();

        Ok(results)
    }

    /// Helper function to extract a snippet of text around the first match of a query.
    fn get_match_context(transcript: &str, query: &str) -> String {
        let transcript_lower = transcript.to_lowercase();
        let query_lower = query.to_lowercase();

        match transcript_lower.find(&query_lower) {
            Some(match_index) => {
                let start_index = match_index.saturating_sub(100);
                let end_index = (match_index + query.len() + 100).min(transcript.len());

                let mut context = String::new();
                if start_index > 0 {
                    context.push_str("...");
                }
                context.push_str(&transcript[start_index..end_index]);
                if end_index < transcript.len() {
                    context.push_str("...");
                }
                context
            }
            None => transcript.chars().take(200).collect(), // Fallback to the start of the transcript
        }
    }
}

#[cfg(test)]
mod verify_meeting_start_time {
    use super::*;
    use crate::audio::recording_saver::{DeviceInfo, MeetingMetadata};
    use crate::database::manager::DatabaseManager;
    use chrono::Duration;

    #[tokio::test]
    async fn created_at_reflects_recording_start_not_save_time() {
        let tmp = std::env::temp_dir().join(format!("meetily-verify-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&tmp).unwrap();

        // Simulate what recording_saver::initialize_meeting_folder writes at the
        // moment recording actually starts: a metadata.json with a created_at
        // timestamp well in the past relative to "now" (the save-time bug this
        // fix addresses).
        let recording_start = Utc::now() - Duration::minutes(37);
        let metadata = MeetingMetadata {
            version: "1.0".to_string(),
            meeting_id: None,
            meeting_name: Some("Verify Meeting".to_string()),
            created_at: recording_start.to_rfc3339(),
            completed_at: Some(Utc::now().to_rfc3339()),
            duration_seconds: Some(120.0),
            devices: DeviceInfo { microphone: None, system_audio: None },
            audio_file: "audio.mp4".to_string(),
            transcript_file: "transcripts.json".to_string(),
            sample_rate: 48000,
            status: "completed".to_string(),
        };
        std::fs::write(
            tmp.join("metadata.json"),
            serde_json::to_string_pretty(&metadata).unwrap(),
        )
        .unwrap();

        let db_path = tmp.join("test.sqlite");
        let db_manager = DatabaseManager::new(
            db_path.to_str().unwrap(),
            tmp.join("nonexistent-legacy.db").to_str().unwrap(),
        )
        .await
        .unwrap();
        let pool = db_manager.pool();

        let before_save = Utc::now();
        let meeting_id = TranscriptsRepository::save_transcript(
            pool,
            "Verify Meeting",
            &[],
            Some(tmp.to_str().unwrap().to_string()),
        )
        .await
        .unwrap();
        let after_save = Utc::now();

        let (created_at, updated_at): (DateTime<Utc>, DateTime<Utc>) =
            sqlx::query_as("SELECT created_at, updated_at FROM meetings WHERE id = ?")
                .bind(&meeting_id)
                .fetch_one(pool)
                .await
                .unwrap();

        println!(
            "recording_start={} created_at={} updated_at={} save_window=[{}, {}]",
            recording_start, created_at, updated_at, before_save, after_save
        );

        // The bug: created_at used to be stamped with save-time `Utc::now()`,
        // which would fall inside [before_save, after_save] — 37 minutes after
        // the real recording start. Assert it instead matches metadata.json.
        assert!(
            (created_at - recording_start).num_seconds().abs() < 2,
            "created_at ({}) should match metadata.json recording start ({}), not save time",
            created_at,
            recording_start
        );
        assert!(
            created_at < before_save - Duration::minutes(30),
            "created_at ({}) should predate the save-time window, proving it is NOT save-time Utc::now()",
            created_at
        );
        // updated_at should still reflect the actual save moment.
        assert!(
            updated_at >= before_save && updated_at <= after_save,
            "updated_at ({}) should fall within the save-time window [{}, {}]",
            updated_at,
            before_save,
            after_save
        );

        std::fs::remove_dir_all(&tmp).ok();
    }

    #[tokio::test]
    async fn falls_back_to_now_when_no_folder_path() {
        let tmp = std::env::temp_dir().join(format!("meetily-verify-{}", Uuid::new_v4()));
        let db_path = tmp.join("test.sqlite");
        let db_manager = DatabaseManager::new(
            db_path.to_str().unwrap(),
            tmp.join("nonexistent-legacy.db").to_str().unwrap(),
        )
        .await
        .unwrap();
        let pool = db_manager.pool();

        let before_save = Utc::now();
        let meeting_id =
            TranscriptsRepository::save_transcript(pool, "No Folder Meeting", &[], None)
                .await
                .unwrap();
        let after_save = Utc::now();

        let (created_at,): (DateTime<Utc>,) =
            sqlx::query_as("SELECT created_at FROM meetings WHERE id = ?")
                .bind(&meeting_id)
                .fetch_one(pool)
                .await
                .unwrap();

        assert!(
            created_at >= before_save && created_at <= after_save,
            "with no folder_path, created_at ({}) should fall back to save-time now [{}, {}]",
            created_at,
            before_save,
            after_save
        );

        std::fs::remove_dir_all(&tmp).ok();
    }
}
