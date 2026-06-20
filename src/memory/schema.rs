pub(super) const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS memory_sources (
    id TEXT PRIMARY KEY,
    source_type TEXT NOT NULL,
    source_ref TEXT NOT NULL,
    trust_level TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS memory_raw_events (
    id TEXT PRIMARY KEY,
    source_id TEXT NOT NULL REFERENCES memory_sources(id),
    event_type TEXT NOT NULL,
    role TEXT NOT NULL,
    trust_level TEXT NOT NULL,
    subject_type TEXT NOT NULL,
    subject_ref TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    confidence REAL NOT NULL,
    occurred_at TEXT NOT NULL,
    created_at TEXT NOT NULL,
    tombstoned_at TEXT
);

CREATE TABLE IF NOT EXISTS memory_nodes (
    id TEXT PRIMARY KEY,
    node_type TEXT NOT NULL,
    raw_event_id TEXT REFERENCES memory_raw_events(id),
    entity_type TEXT,
    entity_value TEXT,
    normalized_value TEXT,
    text_ref TEXT,
    metadata_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    tombstoned_at TEXT
);

CREATE TABLE IF NOT EXISTS memory_node_state (
    node_id TEXT PRIMARY KEY REFERENCES memory_nodes(id) ON DELETE CASCADE,
    salience REAL NOT NULL,
    strength REAL NOT NULL,
    resource REAL NOT NULL,
    recall_count INTEGER NOT NULL,
    reinforce_count INTEGER NOT NULL,
    fan_in INTEGER NOT NULL,
    fan_out INTEGER NOT NULL,
    last_recalled_at TEXT,
    last_reinforced_at TEXT,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS memory_indexes (
    node_id TEXT NOT NULL REFERENCES memory_nodes(id) ON DELETE CASCADE,
    index_type TEXT NOT NULL,
    index_ref_or_payload TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (node_id, index_type, index_ref_or_payload)
);

CREATE TABLE IF NOT EXISTS memory_edges (
    id TEXT PRIMARY KEY,
    from_node_id TEXT NOT NULL REFERENCES memory_nodes(id),
    to_node_id TEXT NOT NULL REFERENCES memory_nodes(id),
    relation TEXT NOT NULL,
    strength REAL NOT NULL,
    confidence REAL NOT NULL,
    evidence_event_ids_json TEXT NOT NULL,
    last_activated_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    tombstoned_at TEXT
);

CREATE TABLE IF NOT EXISTS memory_activation_runs (
    id TEXT PRIMARY KEY,
    query_kind TEXT NOT NULL,
    query_ref TEXT NOT NULL,
    query_json TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS memory_activation_items (
    run_id TEXT NOT NULL REFERENCES memory_activation_runs(id) ON DELETE CASCADE,
    node_id TEXT NOT NULL REFERENCES memory_nodes(id),
    source_channel TEXT NOT NULL,
    direct_score REAL NOT NULL,
    ripple_score REAL NOT NULL,
    salience_score REAL NOT NULL,
    strength_score REAL NOT NULL,
    recency_score REAL NOT NULL,
    resource_penalty REAL NOT NULL,
    hub_penalty REAL NOT NULL,
    role_trust_penalty REAL NOT NULL,
    hop_penalty REAL NOT NULL,
    stale_penalty REAL NOT NULL,
    final_score REAL NOT NULL,
    rank INTEGER NOT NULL,
    explanation TEXT NOT NULL,
    PRIMARY KEY (run_id, rank)
);

CREATE TABLE IF NOT EXISTS memory_writebacks (
    id TEXT PRIMARY KEY,
    activation_run_id TEXT NOT NULL REFERENCES memory_activation_runs(id),
    node_id TEXT NOT NULL REFERENCES memory_nodes(id),
    action TEXT NOT NULL,
    before_json TEXT NOT NULL,
    after_json TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS memory_dream_runs (
    id TEXT PRIMARY KEY,
    trigger TEXT NOT NULL,
    scope TEXT NOT NULL,
    input_activation_run_ids_json TEXT NOT NULL,
    model_status TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS memory_dreams (
    id TEXT PRIMARY KEY,
    dream_run_id TEXT NOT NULL REFERENCES memory_dream_runs(id),
    dream_type TEXT NOT NULL,
    summary TEXT NOT NULL,
    evidence_node_ids_json TEXT NOT NULL,
    evidence_event_ids_json TEXT NOT NULL,
    evidence_hint_ids_json TEXT NOT NULL,
    status TEXT NOT NULL,
    confidence REAL NOT NULL,
    version INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    reviewed_at TEXT
);

CREATE TABLE IF NOT EXISTS memory_hints (
    id TEXT PRIMARY KEY,
    dream_id TEXT NOT NULL REFERENCES memory_dreams(id),
    hint_type TEXT NOT NULL,
    scope_type TEXT NOT NULL,
    scope_ref TEXT NOT NULL,
    summary TEXT NOT NULL,
    policy_json TEXT NOT NULL,
    weight REAL NOT NULL,
    status TEXT NOT NULL,
    created_at TEXT NOT NULL,
    approved_at TEXT,
    expires_at TEXT
);

CREATE TABLE IF NOT EXISTS memory_hint_status_changes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    hint_id TEXT NOT NULL REFERENCES memory_hints(id) ON DELETE CASCADE,
    from_status TEXT NOT NULL,
    to_status TEXT NOT NULL,
    changed_at TEXT NOT NULL,
    reason TEXT
);

CREATE INDEX IF NOT EXISTS idx_memory_raw_events_source_id
    ON memory_raw_events(source_id);
CREATE INDEX IF NOT EXISTS idx_memory_raw_events_subject
    ON memory_raw_events(subject_type, subject_ref);
CREATE INDEX IF NOT EXISTS idx_memory_nodes_raw_event_id
    ON memory_nodes(raw_event_id);
CREATE INDEX IF NOT EXISTS idx_memory_nodes_entity
    ON memory_nodes(entity_type, normalized_value);
CREATE INDEX IF NOT EXISTS idx_memory_edges_from_node
    ON memory_edges(from_node_id);
CREATE INDEX IF NOT EXISTS idx_memory_edges_to_node
    ON memory_edges(to_node_id);
CREATE INDEX IF NOT EXISTS idx_memory_activation_items_node
    ON memory_activation_items(node_id);
CREATE INDEX IF NOT EXISTS idx_memory_dreams_run
    ON memory_dreams(dream_run_id);
CREATE INDEX IF NOT EXISTS idx_memory_hints_dream
    ON memory_hints(dream_id);
"#;
