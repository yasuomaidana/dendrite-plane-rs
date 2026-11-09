use std::collections::HashMap;

use serde::Deserialize;

use crate::client::Client;
use crate::error::{Error, Result};
use crate::types::{Issue, Label, UpdateIssue};

#[derive(Deserialize)]
#[serde(untagged)]
enum LabelsResponse {
    Paged { results: Vec<Label> },
    Bare(Vec<Label>),
}

impl Client {
    /// Every label of the project.
    pub fn list_labels(&self) -> Result<Vec<Label>> {
        let url = self.project_url(true, "labels/");
        let resp: LabelsResponse = self.get(&url, "list labels")?;
        Ok(match resp {
            LabelsResponse::Paged { results } => results,
            LabelsResponse::Bare(list) => list,
        })
    }

    /// Returns the existing label with that exact name, otherwise POSTs one. Idempotent.
    pub fn ensure_label(&self, name: &str, color: &str) -> Result<Label> {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Err(Error::Invalid("label name cannot be empty".into()));
        }
        let existing = self.list_labels()?;
        if let Some(found) = existing.into_iter().find(|l| l.name == trimmed) {
            return Ok(found);
        }
        let c = if color.trim().is_empty() {
            "#7C3AED"
        } else {
            color.trim()
        };
        let body = serde_json::json!({
            "name": trimmed,
            "color": c,
        });
        let url = self.project_url(true, "labels/");
        let (status, text) = self.send("POST", &url, body)?;
        if status == 409 {
            let existing = self.list_labels()?;
            if let Some(found) = existing.into_iter().find(|l| l.name == trimmed) {
                return Ok(found);
            }
        }
        Self::check(status, "create label")?;
        Self::decode(&text)
    }

    /// Reads the issue, maps its label ids to names, drops every label whose name starts with any
    /// remove prefix, adds the `add` names (ensure_label each, default colour "#7C3AED"), PATCHes
    /// the resulting full list, and returns the issue and final label names. Re-applying the same
    /// call will not duplicate labels.
    pub fn patch_labels(
        &self,
        reference: &str,
        add: &[String],
        remove_prefixes: &[String],
    ) -> Result<(Issue, Vec<String>)> {
        let r = reference.trim();
        if r.is_empty() {
            return Err(Error::Invalid("ticket reference cannot be empty".into()));
        }
        let issue = self.get_issue(r)?;
        let all_labels = self.list_labels()?;
        let mut current_names = current_label_names(&issue, &all_labels);
        apply_patch(&mut current_names, add, remove_prefixes);

        // Labels the project already has are used by id; only the missing ones are created.
        let name_to_id: HashMap<&str, &str> = all_labels
            .iter()
            .map(|l| (l.name.as_str(), l.id.as_str()))
            .collect();
        let mut label_ids = Vec::with_capacity(current_names.len());
        for name in &current_names {
            match name_to_id.get(name.as_str()) {
                Some(id) => label_ids.push((*id).to_string()),
                None => label_ids.push(self.ensure_label(name, "#7C3AED")?.id),
            }
        }

        let upd = UpdateIssue {
            labels: Some(label_ids),
            ..UpdateIssue::default()
        };
        let updated = self.update_issue(r, &upd)?;
        Ok((updated, current_names))
    }
}

/// The distinct names of the labels on `issue`, in order. Ids are resolved through the project's
/// labels and the issue's own label details; with no ids at all, the details' names are used.
fn current_label_names(issue: &Issue, all_labels: &[Label]) -> Vec<String> {
    let mut id_to_name: HashMap<&str, &str> = all_labels
        .iter()
        .map(|l| (l.id.as_str(), l.name.as_str()))
        .collect();
    let details = || issue.label_details.iter().filter(|d| !d.name.is_empty());
    id_to_name.extend(
        details()
            .filter(|d| !d.id.is_empty())
            .map(|d| (d.id.as_str(), d.name.as_str())),
    );
    let ids = issue
        .labels
        .iter()
        .filter_map(|id| id_to_name.get(id.as_str()));
    let names: Vec<&str> = if issue.labels.is_empty() {
        details().map(|d| d.name.as_str()).collect()
    } else {
        ids.copied().collect()
    };
    let mut out: Vec<String> = Vec::new();
    for name in names {
        if !out.iter().any(|n| n == name) {
            out.push(name.to_string());
        }
    }
    out
}

/// Drops the names starting with any non-empty remove prefix, then appends the trimmed, non-empty
/// `add` names that are not present yet.
fn apply_patch(names: &mut Vec<String>, add: &[String], remove_prefixes: &[String]) {
    names.retain(|name| {
        !remove_prefixes
            .iter()
            .any(|prefix| !prefix.is_empty() && name.starts_with(prefix))
    });
    for trimmed in add.iter().map(|n| n.trim()).filter(|n| !n.is_empty()) {
        if !names.iter().any(|n| n == trimmed) {
            names.push(trimmed.to_string());
        }
    }
}
