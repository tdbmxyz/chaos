-- Links domain: mirrors chaos-domain::links.
-- UUIDs are stored as hyphenated TEXT (readable with the sqlite3 CLI),
-- timestamps as RFC3339 TEXT (chrono's canonical sqlx mapping).

CREATE TABLE collections (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    description TEXT,
    color       TEXT,
    parent_id   TEXT REFERENCES collections(id) ON DELETE SET NULL,
    created_at  TEXT NOT NULL
);

CREATE TABLE links (
    id            TEXT PRIMARY KEY,
    url           TEXT NOT NULL,
    title         TEXT NOT NULL,
    description   TEXT,
    -- Deleting a collection leaves its links as "unsorted".
    collection_id TEXT REFERENCES collections(id) ON DELETE SET NULL,

    -- Flattened chaos-domain::links::ArchiveState.
    archive_state      TEXT NOT NULL DEFAULT 'none'
                       CHECK (archive_state IN ('none', 'pending', 'archived', 'failed')),
    archived_at        TEXT,
    archive_size_bytes INTEGER,
    archive_error      TEXT,

    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE tags (
    id   TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE COLLATE NOCASE
);

CREATE TABLE link_tags (
    link_id TEXT NOT NULL REFERENCES links(id) ON DELETE CASCADE,
    tag_id  TEXT NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    PRIMARY KEY (link_id, tag_id)
);

CREATE INDEX idx_links_collection ON links(collection_id);
CREATE INDEX idx_links_created_at ON links(created_at DESC);
CREATE INDEX idx_link_tags_tag ON link_tags(tag_id);
CREATE INDEX idx_collections_parent ON collections(parent_id);
