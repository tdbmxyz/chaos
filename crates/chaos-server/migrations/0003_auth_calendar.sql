-- Users, sessions, calendars and events.
-- Conventions as before: UUIDs as hyphenated TEXT, timestamps RFC3339 TEXT.

CREATE TABLE users (
    id            TEXT PRIMARY KEY,
    username      TEXT NOT NULL UNIQUE COLLATE NOCASE,
    display_name  TEXT NOT NULL,
    -- PHC string (argon2id). Empty when the user can only log in through
    -- an external identity provider (authentik, later).
    password_hash TEXT NOT NULL,
    created_at    TEXT NOT NULL
);

CREATE TABLE sessions (
    -- sha256 of the opaque token; the token itself never touches disk.
    token_hash TEXT PRIMARY KEY,
    user_id    TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at TEXT NOT NULL,
    expires_at TEXT NOT NULL
);
CREATE INDEX idx_sessions_user ON sessions(user_id);

CREATE TABLE calendars (
    id         TEXT PRIMARY KEY,
    user_id    TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name       TEXT NOT NULL,
    color      TEXT,
    kind       TEXT NOT NULL CHECK (kind IN ('local', 'ics')),
    ics_url    TEXT,
    created_at TEXT NOT NULL,
    CHECK (kind != 'ics' OR ics_url IS NOT NULL)
);
CREATE INDEX idx_calendars_user ON calendars(user_id);

CREATE TABLE events (
    id          TEXT PRIMARY KEY,
    calendar_id TEXT NOT NULL REFERENCES calendars(id) ON DELETE CASCADE,
    title       TEXT NOT NULL,
    description TEXT,
    location    TEXT,
    starts_at   TEXT NOT NULL,
    ends_at     TEXT NOT NULL,
    all_day     INTEGER NOT NULL DEFAULT 0,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);
CREATE INDEX idx_events_calendar_start ON events(calendar_id, starts_at);
