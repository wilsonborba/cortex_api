CREATE TABLE IF NOT EXISTS research_subjects (
    slug TEXT PRIMARY KEY CHECK (slug <> ''),
    name TEXT NOT NULL CHECK (name <> '')
);

CREATE TABLE IF NOT EXISTS research_sources (
    host TEXT PRIMARY KEY CHECK (host <> ''),
    name TEXT NOT NULL CHECK (name <> ''),
    url TEXT NOT NULL CHECK (url <> ''),
    description TEXT NOT NULL DEFAULT '',
    languages TEXT NOT NULL DEFAULT '',
    regions TEXT NOT NULL DEFAULT '',
    access_note TEXT NOT NULL DEFAULT '',
    access_mode TEXT NOT NULL DEFAULT 'public_page'
        CHECK (access_mode IN ('public_page', 'public_api', 'credentialed_api', 'unavailable')),
    is_enabled BOOLEAN NOT NULL DEFAULT TRUE,
    min_spacing_ms INTEGER NOT NULL DEFAULT 1500 CHECK (min_spacing_ms >= 0),
    max_requests_per_run INTEGER NOT NULL DEFAULT 3 CHECK (max_requests_per_run >= 0),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS research_source_subjects (
    host TEXT NOT NULL REFERENCES research_sources(host) ON DELETE CASCADE,
    subject_slug TEXT NOT NULL REFERENCES research_subjects(slug) ON DELETE CASCADE,
    PRIMARY KEY (host, subject_slug)
);

CREATE TABLE IF NOT EXISTS research_operational_settings (
    setting_key TEXT PRIMARY KEY CHECK (setting_key <> ''),
    setting_value TEXT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
