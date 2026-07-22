-- Per-user post engagement, system post ingestion timestamps, and a generic
-- analytics event log. Conventions per db.rs: UUIDs as hyphenated TEXT,
-- timestamps as RFC3339 TEXT, FKs to users(id).

-- Per-user, per-post engagement. First-occurrence timestamps; a set column
-- means that signal is on. Opening always also sets seen_at.
CREATE TABLE post_views (
    user_id            TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    source             TEXT NOT NULL,          -- "hackernews" | "lobsters"
    post_id            TEXT NOT NULL,          -- provider id
    seen_at            TEXT,
    opened_comments_at TEXT,
    opened_article_at  TEXT,
    updated_at         TEXT NOT NULL,
    PRIMARY KEY (user_id, source, post_id)
);
CREATE INDEX idx_post_views_user ON post_views(user_id);

-- System-wide post ingestion: when a (source, post_id) first entered our DB.
CREATE TABLE posts (
    source        TEXT NOT NULL,
    post_id       TEXT NOT NULL,
    title         TEXT NOT NULL,
    first_seen_at TEXT NOT NULL,
    PRIMARY KEY (source, post_id)
);

-- Generic append-only analytics log. Named analytics_events because the
-- calendar owns the `events` table (see 0003_auth_calendar.sql).
CREATE TABLE analytics_events (
    id      TEXT PRIMARY KEY,
    user_id TEXT REFERENCES users(id) ON DELETE SET NULL,  -- NULL = anonymous
    kind    TEXT NOT NULL,      -- login | app_open | search | reader_open | …
    at      TEXT NOT NULL,
    detail  TEXT               -- free-form: user-agent, query, "source:post_id"
);
CREATE INDEX idx_analytics_events_kind_at ON analytics_events(kind, at);
CREATE INDEX idx_analytics_events_user ON analytics_events(user_id, at);
