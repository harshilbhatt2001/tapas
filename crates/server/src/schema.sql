-- Single user, no migrations: change this file and recreate the database.

-- Weekly series a controlled goal can steer. User-created series are always fed manually.
CREATE TABLE IF NOT EXISTS series (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    unit TEXT NOT NULL,
    aggregation TEXT NOT NULL CHECK (aggregation IN ('level', 'sum')),
    feed TEXT NOT NULL CHECK (feed IN ('manual', 'body_weight', 'training_hours'))
);
INSERT OR IGNORE INTO series (id, name, unit, aggregation, feed) VALUES
    (1, 'Body weight', 'kg', 'level', 'body_weight'),
    (2, 'Training hours', 'h', 'sum', 'training_hours');

-- One value per day for a manually fed series.
CREATE TABLE IF NOT EXISTS manual_entries (
    series_id INTEGER NOT NULL REFERENCES series (id) ON DELETE CASCADE,
    day TEXT NOT NULL,
    value REAL NOT NULL,
    PRIMARY KEY (series_id, day)
) WITHOUT ROWID;

-- The goal tree. The one master goal is the root; every other goal has a parent, and a
-- checkpoint is just a child goal due before its parent.
CREATE TABLE IF NOT EXISTS goals (
    id INTEGER PRIMARY KEY,
    parent_id INTEGER REFERENCES goals (id) ON DELETE CASCADE,
    title TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('master', 'controlled', 'milestone')),
    due TEXT,
    start TEXT,
    series_id INTEGER REFERENCES series (id) ON DELETE SET NULL,
    start_value REAL,
    target_value REAL,
    done_on TEXT,
    weight REAL NOT NULL DEFAULT 1,
    weekly_hours REAL,
    capacity_hours REAL,
    max_ramp REAL,
    CHECK ((kind = 'master') = (parent_id IS NULL))
);
CREATE UNIQUE INDEX IF NOT EXISTS goals_one_master ON goals (kind) WHERE kind = 'master';

CREATE TABLE IF NOT EXISTS measurements (
    metric TEXT NOT NULL,
    source TEXT NOT NULL,
    at TEXT NOT NULL,
    value REAL NOT NULL,
    PRIMARY KEY (metric, source, at)
) WITHOUT ROWID;

-- A training week is not stored: it is the ISO-week view of a goal's workouts.
CREATE TABLE IF NOT EXISTS workouts (
    source TEXT NOT NULL,
    source_id TEXT NOT NULL,
    activity TEXT NOT NULL,
    start TEXT NOT NULL,
    end TEXT NOT NULL,
    active_secs INTEGER,
    distance_m REAL,
    avg_hr INTEGER,
    kcal REAL,
    goal_id INTEGER REFERENCES goals (id) ON DELETE SET NULL,
    PRIMARY KEY (source, source_id)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS workouts_by_start ON workouts (start);

-- One Google consent per API; the refresh token is encrypted with the server's token key.
CREATE TABLE IF NOT EXISTS google_grants (
    api TEXT PRIMARY KEY,
    sealed_refresh_token BLOB NOT NULL,
    granted_at TEXT NOT NULL,
    synced_at TEXT,
    sync_error TEXT,
    -- Google refused the refresh token: only a new consent in the app helps.
    needs_consent INTEGER NOT NULL DEFAULT 0
);

-- App sessions, stored as SHA-256 of the bearer token.
CREATE TABLE IF NOT EXISTS sessions (
    token_hash BLOB PRIMARY KEY,
    expires_at TEXT NOT NULL
);
