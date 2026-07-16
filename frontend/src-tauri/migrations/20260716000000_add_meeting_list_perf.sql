-- Speed up the sidebar meeting list, which previously did an unindexed
-- LEFT JOIN against transcripts (the largest table) plus a MIN/MAX
-- aggregation on every fetch.

-- transcripts.meeting_id had no index, so the JOIN was a full table scan.
CREATE INDEX IF NOT EXISTS idx_transcripts_meeting_id ON transcripts(meeting_id);

-- meetings.created_at had no index despite being the sort key for every list fetch.
CREATE INDEX IF NOT EXISTS idx_meetings_created_at ON meetings(created_at);

-- Denormalize recording duration onto meetings so the list query no longer
-- needs to JOIN/aggregate transcripts at read time. Populated going forward
-- at write time (see save_transcript / create_meeting_with_transcripts).
ALTER TABLE meetings ADD COLUMN duration_seconds REAL;

-- Backfill duration for existing meetings from their transcript segments.
UPDATE meetings
SET duration_seconds = (
    SELECT MAX(t.audio_end_time) - MIN(t.audio_start_time)
    FROM transcripts t
    WHERE t.meeting_id = meetings.id
)
WHERE EXISTS (SELECT 1 FROM transcripts t WHERE t.meeting_id = meetings.id);
