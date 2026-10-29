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
        let mut id_to_name: HashMap<String, String> = HashMap::new();
        for l in &all_labels {
            id_to_name.insert(l.id.clone(), l.name.clone());
        }
        for d in &issue.label_details {
            if !d.id.is_empty() && !d.name.is_empty() {
                id_to_name.insert(d.id.clone(), d.name.clone());
            }
        }

        let mut current_names: Vec<String> = Vec::new();
        for id in &issue.labels {
            if let Some(name) = id_to_name.get(id)
                && !current_names.contains(name)
            {
                current_names.push(name.clone());
            }
        }
        if current_names.is_empty() && issue.labels.is_empty() {
            for d in &issue.label_details {
                if !d.name.is_empty() && !current_names.contains(&d.name) {
                    current_names.push(d.name.clone());
                }
            }
        }

        current_names.retain(|name| {
            !remove_prefixes
                .iter()
                .any(|prefix| !prefix.is_empty() && name.starts_with(prefix))
        });

        for name in add {
            let trimmed = name.trim();
            if trimmed.is_empty() {
                continue;
            }
            if !current_names.iter().any(|n| n == trimmed) {
                current_names.push(trimmed.to_string());
            }
        }

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
