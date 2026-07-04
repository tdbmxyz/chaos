-- Full-text index over archived page content. Populated by the archiver
-- after a successful monolith snapshot; searched from list_links (q filter).
CREATE VIRTUAL TABLE archive_fts USING fts5(
    link_id UNINDEXED,
    content,
    tokenize = 'porter unicode61'
);
