-- Session-scoped side context supplied by the operator through `/btw`.
-- A JSON array, following the pending_tasks/changed_files precedent in 0001.
--
-- These notes are compiled into the prompt as their own context section and
-- are deliberately NOT rows in `messages`: that is what keeps them out of the
-- conversation history the model re-reads (and pays for) on every turn, and
-- out of reach of compaction, which only folds messages.

ALTER TABLE sessions ADD COLUMN side_notes TEXT NOT NULL DEFAULT '[]';
