use std::collections::BTreeSet;

use anyhow::Result;
use serde_json::Value;

use crate::memory::model::{MemoryIndex, MemoryIndexType, MemoryNode, MemoryNodeType};
use crate::memory::store::MemoryStore;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmbeddingIndexStatus {
    Disabled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryIndexBuildReport {
    pub nodes_seen: usize,
    pub nodes_indexed: usize,
    pub indexes_written: usize,
    pub embedding_status: EmbeddingIndexStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryRecallMatch {
    pub node_id: String,
    pub index_type: MemoryIndexType,
    pub matched_payload: String,
}

pub struct MemoryIndexBuilder;

impl MemoryIndexBuilder {
    pub fn rebuild(store: &MemoryStore, created_at: &str) -> Result<MemoryIndexBuildReport> {
        store.clear_indexes()?;
        let nodes = store.list_nodes()?;
        let nodes_seen = nodes.len();
        let mut nodes_indexed = 0;
        let mut indexes_written = 0;

        for node in nodes.iter().filter(|node| node.tombstoned_at.is_none()) {
            let indexes = build_indexes_for_node(node, store, created_at)?;
            if !indexes.is_empty() {
                nodes_indexed += 1;
            }
            for index in indexes {
                store.insert_index(&index)?;
                indexes_written += 1;
            }
        }

        Ok(MemoryIndexBuildReport {
            nodes_seen,
            nodes_indexed,
            indexes_written,
            embedding_status: EmbeddingIndexStatus::Disabled,
        })
    }
}

pub struct MemoryIndexSearch;

impl MemoryIndexSearch {
    pub fn search_fts(store: &MemoryStore, query: &str) -> Result<Vec<MemoryRecallMatch>> {
        search_token_index(store, MemoryIndexType::Fts, &tokens_for_text(query))
    }

    pub fn search_rare_tokens(store: &MemoryStore, query: &str) -> Result<Vec<MemoryRecallMatch>> {
        search_token_index(store, MemoryIndexType::RareToken, &tokens_for_text(query))
    }

    pub fn search_entity(
        store: &MemoryStore,
        entity_type: &str,
        entity_value: &str,
    ) -> Result<Vec<MemoryRecallMatch>> {
        let payload = entity_payload(entity_type, entity_value);
        let indexes = store.find_indexes(MemoryIndexType::Entity, &payload)?;
        Ok(indexes
            .into_iter()
            .map(|index| MemoryRecallMatch {
                node_id: index.node_id,
                index_type: MemoryIndexType::Entity,
                matched_payload: payload.clone(),
            })
            .collect())
    }
}

fn build_indexes_for_node(
    node: &MemoryNode,
    store: &MemoryStore,
    created_at: &str,
) -> Result<Vec<MemoryIndex>> {
    let mut indexes = Vec::new();
    match node.node_type {
        MemoryNodeType::RawEvent => {
            let Some(raw_event_id) = node.raw_event_id.as_deref() else {
                return Ok(indexes);
            };
            let Some(raw_event) = store.get_raw_event(raw_event_id)? else {
                return Ok(indexes);
            };
            if raw_event.tombstoned_at.is_some() {
                return Ok(indexes);
            }

            let mut tokens = BTreeSet::new();
            tokens.extend(tokens_for_text(&raw_event.subject_ref));
            collect_json_tokens(&raw_event.payload_json, &mut tokens);
            for token in &tokens {
                indexes.push(index(node, MemoryIndexType::Fts, token, created_at));
                if is_rare_token(token) {
                    indexes.push(index(node, MemoryIndexType::RareToken, token, created_at));
                }
            }
        }
        MemoryNodeType::Entity => {
            if let (Some(entity_type), Some(value)) = (
                node.entity_type.as_deref(),
                node.normalized_value.as_deref(),
            ) {
                indexes.push(index(
                    node,
                    MemoryIndexType::Entity,
                    &entity_payload(entity_type, value),
                    created_at,
                ));
            }
        }
        MemoryNodeType::Episode
        | MemoryNodeType::ClaimCandidate
        | MemoryNodeType::Dream
        | MemoryNodeType::Hint => {}
    }
    Ok(indexes)
}

fn index(
    node: &MemoryNode,
    index_type: MemoryIndexType,
    payload: &str,
    created_at: &str,
) -> MemoryIndex {
    MemoryIndex {
        node_id: node.id.clone(),
        index_type,
        index_ref_or_payload: payload.to_string(),
        created_at: created_at.to_string(),
    }
}

fn search_token_index(
    store: &MemoryStore,
    index_type: MemoryIndexType,
    tokens: &BTreeSet<String>,
) -> Result<Vec<MemoryRecallMatch>> {
    let mut seen = BTreeSet::new();
    let mut matches = Vec::new();
    for token in tokens {
        for index in store.find_indexes(index_type, token)? {
            let key = format!("{}:{token}", index.node_id);
            if seen.insert(key) {
                matches.push(MemoryRecallMatch {
                    node_id: index.node_id,
                    index_type,
                    matched_payload: token.clone(),
                });
            }
        }
    }
    Ok(matches)
}

fn collect_json_tokens(value: &Value, tokens: &mut BTreeSet<String>) {
    match value {
        Value::String(text) => tokens.extend(tokens_for_text(text)),
        Value::Array(items) => {
            for item in items {
                collect_json_tokens(item, tokens);
            }
        }
        Value::Object(object) => {
            for (key, value) in object {
                tokens.extend(tokens_for_text(key));
                collect_json_tokens(value, tokens);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

fn tokens_for_text(text: &str) -> BTreeSet<String> {
    let mut tokens = BTreeSet::new();
    for chunk in text.split_whitespace() {
        let literal = normalize_literal(chunk.trim_matches(|ch: char| {
            !ch.is_ascii_alphanumeric() && ch != '_' && ch != '-' && ch != '/' && ch != '#'
        }));
        if literal.len() >= 2 {
            tokens.insert(literal.clone());
        }
        if let Some((repo, _issue)) = literal.split_once('#') {
            if !repo.is_empty() {
                tokens.insert(repo.to_string());
            }
        }
    }

    for raw in text.split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_' && ch != '-') {
        let token = raw.trim().to_lowercase();
        if token.len() >= 2 {
            tokens.insert(token);
        }
    }

    tokens
}

fn entity_payload(entity_type: &str, entity_value: &str) -> String {
    format!("{}:{}", entity_type.trim(), normalize_literal(entity_value))
}

fn normalize_literal(value: &str) -> String {
    value.trim().to_lowercase()
}

fn is_rare_token(token: &str) -> bool {
    token.len() >= 6 || token.contains('_') || token.contains('-') || token.contains('/')
}

#[cfg(test)]
mod tests {
    use super::tokens_for_text;

    #[test]
    fn tokenization_preserves_repo_and_error_like_tokens() {
        let tokens = tokens_for_text("owner/repo#123 panics with E0425 in parser-core");
        assert!(tokens.contains("owner/repo#123"));
        assert!(tokens.contains("owner/repo"));
        assert!(tokens.contains("e0425"));
        assert!(tokens.contains("parser-core"));
    }
}
