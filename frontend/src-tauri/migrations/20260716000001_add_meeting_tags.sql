-- Tags for grouping/filtering meetings by topic or initiative.

CREATE TABLE IF NOT EXISTS tags (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE COLLATE NOCASE,
    color TEXT,
    created_at TEXT NOT NULL
);

-- Many-to-many: a meeting can have several tags, a tag can span many meetings.
CREATE TABLE IF NOT EXISTS meeting_tags (
    meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
    tag_id TEXT NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    created_at TEXT NOT NULL,
    PRIMARY KEY (meeting_id, tag_id)
);

-- meeting_id lookups are covered by the PRIMARY KEY (meeting_id is its
-- leading column); tag_id needs its own index for "all meetings with tag X".
CREATE INDEX IF NOT EXISTS idx_meeting_tags_tag_id ON meeting_tags(tag_id);
