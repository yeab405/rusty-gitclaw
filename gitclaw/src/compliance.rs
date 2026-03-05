use serde::{Deserialize, Serialize};
use serde_yaml;
use std::path::Path;
use tokio::fs;

use crate::loader::AgentManifest;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComplianceWarning {
    pub rule: String,
    pub message: String,
    pub severity: String,
}

pub fn validate_compliance(manifest: &AgentManifest) -> Vec<ComplianceWarning> {
    let mut warnings = Vec::new();
    let compliance = match &manifest.compliance {
        Some(c) => c,
        None => return warnings,
    };

    let risk_level = compliance.get("risk_level").and_then(|v| v.as_str()).unwrap_or("");
    let hitl = compliance.get("human_in_the_loop").and_then(|v| v.as_bool()).unwrap_or(false);
    let has_audit = compliance.get("recordkeeping")
        .and_then(|v| v.get("audit_logging"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let has_retention = compliance.get("recordkeeping")
        .and_then(|v| v.get("retention_days"))
        .is_some();
    let has_recordkeeping = compliance.get("recordkeeping").is_some();
    let has_review = compliance.get("review").is_some();
    let has_frameworks = compliance.get("regulatory_frameworks")
        .and_then(|v| v.as_sequence())
        .map(|s| !s.is_empty())
        .unwrap_or(false);
    let has_classification = compliance.get("data_classification").is_some();

    if (risk_level == "high" || risk_level == "critical") && !hitl {
        warnings.push(ComplianceWarning {
            rule: "high_risk_hitl".to_string(),
            message: format!("Agent with risk_level \"{risk_level}\" should have human_in_the_loop enabled"),
            severity: "warning".to_string(),
        });
    }

    if risk_level == "critical" && !has_audit {
        warnings.push(ComplianceWarning {
            rule: "critical_audit".to_string(),
            message: "Critical risk agents must have recordkeeping.audit_logging enabled".to_string(),
            severity: "error".to_string(),
        });
    }

    if has_frameworks && !has_recordkeeping {
        warnings.push(ComplianceWarning {
            rule: "regulatory_recordkeeping".to_string(),
            message: "Agents with regulatory frameworks should have recordkeeping configured".to_string(),
            severity: "warning".to_string(),
        });
    }

    if (risk_level == "high" || risk_level == "critical") && !has_review {
        warnings.push(ComplianceWarning {
            rule: "high_risk_review".to_string(),
            message: format!("Agent with risk_level \"{risk_level}\" should have review configuration"),
            severity: "warning".to_string(),
        });
    }

    if has_audit && !has_retention {
        warnings.push(ComplianceWarning {
            rule: "audit_retention".to_string(),
            message: "Audit logging enabled but no retention_days specified".to_string(),
            severity: "warning".to_string(),
        });
    }

    if has_frameworks && !has_classification {
        warnings.push(ComplianceWarning {
            rule: "data_classification".to_string(),
            message: "Regulated agents should specify data_classification".to_string(),
            severity: "warning".to_string(),
        });
    }

    warnings
}

pub async fn load_compliance_context(agent_dir: &Path) -> String {
    let compliance_dir = agent_dir.join("compliance");
    let mut parts = Vec::new();

    if let Ok(raw) = fs::read_to_string(compliance_dir.join("regulatory-map.yaml")).await {
        if let Ok(map) = serde_yaml::from_str::<serde_yaml::Value>(&raw) {
            if let Some(frameworks) = map.get("frameworks").and_then(|f| f.as_mapping()) {
                let names: Vec<&str> = frameworks.keys().filter_map(|k| k.as_str()).collect();
                if !names.is_empty() {
                    parts.push(format!("Regulatory frameworks: {}", names.join(", ")));
                }
            }
        }
    }

    if let Ok(raw) = fs::read_to_string(compliance_dir.join("validation-schedule.yaml")).await {
        if let Ok(schedule) = serde_yaml::from_str::<serde_yaml::Value>(&raw) {
            if let Some(checks) = schedule.get("checks").and_then(|c| c.as_sequence()) {
                let check_list: Vec<String> = checks.iter().filter_map(|c| {
                    let name = c.get("name")?.as_str()?;
                    let freq = c.get("frequency")?.as_str()?;
                    let desc = c.get("description").and_then(|d| d.as_str());
                    Some(if let Some(d) = desc {
                        format!("- {name} ({freq}): {d}")
                    } else {
                        format!("- {name} ({freq})")
                    })
                }).collect();
                if !check_list.is_empty() {
                    parts.push(format!("Validation schedule:\n{}", check_list.join("\n")));
                }
            }
        }
    }

    if parts.is_empty() {
        return String::new();
    }
    format!("# Compliance\n\n{}", parts.join("\n\n"))
}

pub fn format_compliance_warnings(warnings: &[ComplianceWarning]) -> String {
    if warnings.is_empty() {
        return String::new();
    }
    warnings
        .iter()
        .map(|w| {
            let icon = if w.severity == "error" { "✗" } else { "⚠" };
            format!("  {icon} [{}] {}", w.rule, w.message)
        })
        .collect::<Vec<_>>()
        .join("\n")
}
