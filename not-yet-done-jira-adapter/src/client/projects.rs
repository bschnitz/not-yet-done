//! Project metadata: the projects themselves, the issue types one offers,
//! and its versions.
//!
//! A type is named, not numbered, everywhere an issue is written (`create`
//! posts `issuetype: { name }`), so what a caller needs from here is the
//! list of names it may use — and a wrong name answered before the POST
//! rather than as a server-side rejection afterwards.
//!
//! Versions are the same story one level up: an issue carries its
//! `fixVersions` as names, and the names alone never say which of them is
//! already shipped. Only the project's own version list knows that
//! (`released`, `releaseDate`, `archived`), so "which release is still
//! open?" is answerable here and nowhere else.

use serde::Deserialize;

use super::JiraClient;

/// One issue type of a project, as `GET /rest/api/2/project/{key}` lists it.
#[derive(Debug, Clone)]
pub struct JiraIssueType {
    pub id: String,
    pub name: String,
    /// Sub-task types cannot stand on their own — a create without a parent
    /// is rejected — so callers that offer a plain "new issue" filter them.
    pub subtask: bool,
}

#[derive(Deserialize)]
struct ProjectResponse {
    id: String,
    #[serde(rename = "issueTypes")]
    issue_types: Option<Vec<RawIssueType>>,
}

#[derive(Deserialize)]
struct RawIssueType {
    id: String,
    name: Option<String>,
    subtask: Option<bool>,
}

impl JiraClient {
    /// The project's id and its issue types. The project key is taken as
    /// given (`PROJ`); Jira accepts the numeric id there just as well.
    pub async fn get_project_issue_types(
        &self,
        project: &str,
    ) -> Result<(String, Vec<JiraIssueType>), String> {
        let url = format!("{}/rest/api/2/project/{}", self.base_url, project);

        let resp = self.send("GET", &url, self.http.get(&url)).await?;
        let resp = self.check_status("GET", &url, resp).await?;
        let body_text = resp
            .text()
            .await
            .map_err(|e| format!("Failed to read response: {e}"))?;

        let data: ProjectResponse = serde_json::from_str(&body_text)
            .map_err(|e| format!("Failed to parse project: {e}"))?;

        let types = data
            .issue_types
            .unwrap_or_default()
            .into_iter()
            .map(|t| JiraIssueType {
                id: t.id,
                name: t.name.unwrap_or_default(),
                subtask: t.subtask.unwrap_or(false),
            })
            .collect();

        Ok((data.id, types))
    }
}

/// A project as `GET /rest/api/2/project` lists it. Only the three fields
/// every caller needs to address one — the full payload carries avatars,
/// categories and lead details nobody here reads.
#[derive(Debug, Clone)]
pub struct JiraProject {
    pub id: String,
    pub key: String,
    pub name: String,
}

#[derive(Deserialize)]
struct RawProject {
    id: String,
    key: String,
    #[serde(default)]
    name: Option<String>,
}

/// One version (Jira's word for a release) of a project, as
/// `GET /rest/api/2/project/{key}/versions` lists it.
///
/// `released` is the flag the fix-version name cannot carry: a version is
/// *done* when someone released it in Jira, not when its date has passed.
/// `archived` versions are kept out of Jira's own pickers and are usually
/// out of the way of the question being asked.
#[derive(Debug, Clone)]
pub struct JiraVersion {
    pub id: String,
    pub name: String,
    pub description: String,
    pub released: bool,
    pub archived: bool,
    /// Planned start, `YYYY-MM-DD`, empty when the project keeps none.
    pub start_date: String,
    /// Planned or actual release date, `YYYY-MM-DD`, empty when unset.
    /// Jira does not distinguish the two here — a released version's date
    /// is the one that was planned unless someone changed it.
    pub release_date: String,
    /// Jira's own verdict: unreleased and past its release date. Absent
    /// from the payload for released versions, where it reads as `false`.
    pub overdue: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawVersion {
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    released: Option<bool>,
    #[serde(default)]
    archived: Option<bool>,
    #[serde(default)]
    start_date: Option<String>,
    #[serde(default)]
    release_date: Option<String>,
    #[serde(default)]
    overdue: Option<bool>,
}

impl JiraClient {
    /// Every project visible to the current user. `GET /rest/api/2/project`
    /// answers in one shot on Jira Server / Data Center — there is no
    /// pagination on this endpoint, which is why this is the one listing in
    /// the client that does not loop.
    pub async fn all_projects(&self) -> Result<Vec<JiraProject>, String> {
        let url = format!("{}/rest/api/2/project", self.base_url);

        let resp = self.send("GET", &url, self.http.get(&url)).await?;
        let resp = self.check_status("GET", &url, resp).await?;
        let body = resp
            .text()
            .await
            .map_err(|e| format!("Failed to read response: {e}"))?;

        let raw: Vec<RawProject> =
            serde_json::from_str(&body).map_err(|e| format!("Failed to parse projects: {e}"))?;

        Ok(raw.into_iter().map(project_from_raw).collect())
    }

    /// A project's versions, in the order the project keeps them — which is
    /// the release *sequence*, not the alphabet, and the only order in which
    /// "the next one" means anything. The project key is taken as given
    /// (`PROJ`); Jira accepts the numeric id there just as well.
    pub async fn project_versions(&self, project: &str) -> Result<Vec<JiraVersion>, String> {
        let url = format!("{}/rest/api/2/project/{}/versions", self.base_url, project);

        let resp = self.send("GET", &url, self.http.get(&url)).await?;
        let resp = self.check_status("GET", &url, resp).await?;
        let body = resp
            .text()
            .await
            .map_err(|e| format!("Failed to read response: {e}"))?;

        let raw: Vec<RawVersion> =
            serde_json::from_str(&body).map_err(|e| format!("Failed to parse versions: {e}"))?;

        Ok(raw.into_iter().map(version_from_raw).collect())
    }
}

fn project_from_raw(p: RawProject) -> JiraProject {
    JiraProject {
        id: p.id,
        key: p.key,
        name: p.name.unwrap_or_default(),
    }
}

fn version_from_raw(v: RawVersion) -> JiraVersion {
    JiraVersion {
        id: v.id,
        name: v.name.unwrap_or_default(),
        description: v.description.unwrap_or_default(),
        released: v.released.unwrap_or(false),
        archived: v.archived.unwrap_or(false),
        start_date: v.start_date.unwrap_or_default(),
        release_date: v.release_date.unwrap_or_default(),
        overdue: v.overdue.unwrap_or(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The payload as Jira Server answers it, including the fields it
    /// leaves out: an unreleased version without dates has neither
    /// `startDate` nor `releaseDate`, and a released one carries no
    /// `overdue`. Every one of those has to read as a value, not as a
    /// parse error — the whole listing would be lost over one absent key.
    #[test]
    fn parses_versions_with_fields_jira_omits() {
        let body = r#"[
          {"id":"1","name":"RL-12-2026","archived":false,"released":true,
           "releaseDate":"2026-03-02","projectId":10000},
          {"id":"2","name":"RL-14-2026","archived":false,"released":false,
           "description":"next one","startDate":"2026-03-03",
           "releaseDate":"2026-04-13","overdue":false,"projectId":10000},
          {"id":"3","name":"RL-99-2026","archived":true,"released":false}
        ]"#;
        let raw: Vec<RawVersion> = serde_json::from_str(body).expect("parses");
        let versions: Vec<JiraVersion> = raw.into_iter().map(version_from_raw).collect();

        assert_eq!(versions.len(), 3);
        assert!(versions[0].released);
        assert_eq!(versions[0].release_date, "2026-03-02");
        assert_eq!(versions[0].start_date, "", "absent startDate reads empty");
        assert!(!versions[0].overdue, "absent overdue reads false");

        assert!(!versions[1].released);
        assert_eq!(versions[1].description, "next one");

        assert!(versions[2].archived);
        assert_eq!(versions[2].release_date, "");
    }

    /// The order Jira answers in is the project's release sequence. It is
    /// the only thing that makes "the next unreleased one" a question with
    /// an answer, so nothing on the way in may re-order the list.
    #[test]
    fn keeps_jiras_own_order() {
        let body = r#"[{"id":"1","name":"RL-14-2026"},{"id":"2","name":"RL-13-2026"}]"#;
        let raw: Vec<RawVersion> = serde_json::from_str(body).expect("parses");
        let names: Vec<String> = raw.into_iter().map(version_from_raw).map(|v| v.name).collect();
        assert_eq!(names, vec!["RL-14-2026", "RL-13-2026"]);
    }

    #[test]
    fn parses_a_project_without_a_name() {
        let body = r#"[{"id":"10000","key":"PROJ","name":"A Project"},{"id":"10001","key":"BARE"}]"#;
        let raw: Vec<RawProject> = serde_json::from_str(body).expect("parses");
        let projects: Vec<JiraProject> = raw.into_iter().map(project_from_raw).collect();
        assert_eq!(projects[0].name, "A Project");
        assert_eq!(projects[1].key, "BARE");
        assert_eq!(projects[1].name, "");
    }
}
