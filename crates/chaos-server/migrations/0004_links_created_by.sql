-- Attribution only, not access control: every link may still be seen and
-- edited by any user, this just records who added it (e.g. for imports).
ALTER TABLE links ADD COLUMN created_by TEXT REFERENCES users(id) ON DELETE SET NULL;
CREATE INDEX idx_links_created_by ON links(created_by);
