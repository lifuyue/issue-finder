use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::github_enrichment::EnrichedIssue;
use crate::repo_scan::RepoScan;
use crate::value_scoring::ValueAssessment;
use crate::value_signals::SignalAxis;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EvidencePack {
    pub why_this_has_high_attention: Vec<EvidenceItem>,
    pub why_this_is_agent_ready: Vec<EvidenceItem>,
    pub risk_factors: Vec<EvidenceItem>,
    pub missing_evidence: Vec<String>,
    pub source_refs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EvidenceItem {
    pub summary: String,
    pub source_refs: Vec<String>,
}

impl EvidencePack {
    pub fn empty() -> Self {
        Self {
            why_this_has_high_attention: Vec::new(),
            why_this_is_agent_ready: Vec::new(),
            risk_factors: Vec::new(),
            missing_evidence: Vec::new(),
            source_refs: Vec::new(),
        }
    }

    pub fn dedupe_source_refs(&mut self) {
        self.source_refs = dedupe_refs(self.source_refs.clone());
        for item in self
            .why_this_has_high_attention
            .iter_mut()
            .chain(self.why_this_is_agent_ready.iter_mut())
            .chain(self.risk_factors.iter_mut())
        {
            item.source_refs = dedupe_refs(item.source_refs.clone());
        }
    }

    pub fn has_complete_item_refs(&self) -> bool {
        self.why_this_has_high_attention
            .iter()
            .chain(self.why_this_is_agent_ready.iter())
            .chain(self.risk_factors.iter())
            .all(|item| !item.source_refs.is_empty())
    }
}

pub fn build_evidence_pack(
    assessment: &ValueAssessment,
    enriched: &EnrichedIssue,
    scan: Option<&RepoScan>,
) -> EvidencePack {
    let mut pack = EvidencePack::empty();

    for signal in &assessment.signals {
        let item = EvidenceItem {
            summary: signal.summary.clone(),
            source_refs: signal.evidence_refs.clone(),
        };
        match signal.axis {
            SignalAxis::Attention | SignalAxis::ProfileFit => {
                pack.why_this_has_high_attention.push(item)
            }
            SignalAxis::Execution => pack.why_this_is_agent_ready.push(item),
        }
    }

    for item in &assessment.evidence {
        let evidence_item = EvidenceItem {
            summary: item.summary.clone(),
            source_refs: item.evidence_refs.clone(),
        };
        if item.summary.contains("Execution quality") || item.summary.contains("Profile fit") {
            pack.why_this_is_agent_ready.push(evidence_item);
        } else if item.summary.contains("risk") || item.summary.contains("hard_fail") {
            pack.risk_factors.push(evidence_item);
        } else {
            pack.why_this_has_high_attention.push(evidence_item);
        }
    }

    if let Some(scan) = scan {
        if !scan.candidate_files.is_empty() {
            pack.why_this_is_agent_ready.push(EvidenceItem {
                summary: format!(
                    "Repository scan found {} candidate file(s) for initial inspection.",
                    scan.candidate_files.len()
                ),
                source_refs: vec!["repo_scan:candidate_files".to_string()],
            });
        }
        if !scan.validation_commands.is_empty() {
            pack.why_this_is_agent_ready.push(EvidenceItem {
                summary: format!(
                    "Repository scan detected {} suggested validation command(s).",
                    scan.validation_commands.len()
                ),
                source_refs: vec!["repo_scan:validation_commands".to_string()],
            });
        }
        for warning in &scan.warnings {
            pack.risk_factors.push(EvidenceItem {
                summary: warning.clone(),
                source_refs: vec!["repo_scan:warnings".to_string()],
            });
        }
    }

    for tag in &assessment.risk_tags {
        pack.risk_factors.push(EvidenceItem {
            summary: format!("Risk tag: {tag}"),
            source_refs: vec!["value_assessment:risk_tags".to_string()],
        });
    }

    for warning in &enriched.warnings {
        pack.risk_factors.push(EvidenceItem {
            summary: warning.clone(),
            source_refs: vec!["enrichment:warnings".to_string()],
        });
    }

    pack.missing_evidence = assessment.missing_evidence.clone();
    pack.source_refs = collect_source_refs(&pack);
    pack.dedupe_source_refs();
    pack
}

pub fn collect_source_refs(pack: &EvidencePack) -> Vec<String> {
    let mut refs = Vec::new();
    for item in pack
        .why_this_has_high_attention
        .iter()
        .chain(pack.why_this_is_agent_ready.iter())
        .chain(pack.risk_factors.iter())
    {
        refs.extend(item.source_refs.clone());
    }
    refs
}

pub fn dedupe_refs(values: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut deduped = Vec::new();
    for value in values {
        if !value.trim().is_empty() && seen.insert(value.clone()) {
            deduped.push(value);
        }
    }
    deduped
}
