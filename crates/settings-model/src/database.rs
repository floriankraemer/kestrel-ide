//! Settings > Database (Database Tools plan F1.4, database-tools.md §8):
//! the one-source-at-a-time draft the data-source dialog edits, and
//! validation for the fields a plain string or number field cannot
//! self-check.
//!
//! Shaped after [`crate::build_tools`]: persistence stays in `app-config`
//! (`DataSourceSetting`), and this module owns the draft/problem
//! vocabulary both the dialog and its Rust-side editor (`ui-shell`'s
//! `DataSourceEditor`, F1.6) program against. Unlike a whole-page draft,
//! one [`DataSourceDraft`] is one source — the dialog's "Add"/"Edit" flow,
//! not the settings page's own list (that list is `Vec<DataSourceSetting>`,
//! read and reordered directly).

use app_config::database::{DataSourceSetting, SshSetting, SslSetting};

/// One field the dialog can flag a problem against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataSourceField {
    Name,
    Id,
    Port,
    Color,
    SshUser,
    FileSourcePath,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataSourceProblem {
    pub field: DataSourceField,
    pub sentence: String,
}

/// The dialog's draft, committed to a [`DataSourceSetting`] on OK.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DataSourceDraft {
    pub id: String,
    pub name: String,
    pub driver: String,
    pub group: String,
    pub color: String,
    pub host: String,
    pub port: Option<u16>,
    pub database: String,
    pub user: String,
    pub auth: String,
    pub read_only: bool,
    pub history: bool,
    pub url: String,
    pub ssl_mode: String,
    pub ssl_ca_file: String,
    pub ssh_host: String,
    pub ssh_port: Option<u16>,
    pub ssh_user: String,
    pub ssh_auth: String,
    pub ssh_key_file: String,
}

impl DataSourceDraft {
    /// A fresh draft for "Add", `id` already assigned by the caller (a
    /// freshly generated uuid — the same shape a new run configuration or
    /// container connection is handed one at creation, not left blank).
    pub fn new(id: String, driver: String) -> Self {
        Self {
            id,
            driver,
            history: true,
            ssl_mode: "prefer".to_string(),
            ..Default::default()
        }
    }

    /// The draft an "Edit" opens with.
    pub fn from_setting(setting: &DataSourceSetting) -> Self {
        let ssh = setting.ssh.clone().unwrap_or_default();
        Self {
            id: setting.id.clone(),
            name: setting.name.clone(),
            driver: setting.driver.clone(),
            group: setting.group.clone(),
            color: setting.color.clone(),
            host: setting.host.clone(),
            port: setting.port,
            database: setting.database.clone(),
            user: setting.user.clone(),
            auth: setting.auth.clone(),
            read_only: setting.read_only,
            history: setting.history,
            url: setting.url.clone(),
            ssl_mode: setting.ssl.mode.clone(),
            ssl_ca_file: setting.ssl.ca_file.clone(),
            ssh_host: ssh.host,
            ssh_port: ssh.port,
            ssh_user: ssh.user,
            ssh_auth: ssh.auth,
            ssh_key_file: ssh.key_file,
        }
    }

    /// `Some` only once the SSH host is non-empty — an SSH tunnel with an
    /// empty host is "not configured", not "configured with no host".
    fn ssh_setting(&self) -> Option<SshSetting> {
        if self.ssh_host.is_empty() {
            return None;
        }
        Some(SshSetting {
            host: self.ssh_host.clone(),
            port: self.ssh_port,
            user: self.ssh_user.clone(),
            auth: self.ssh_auth.clone(),
            key_file: self.ssh_key_file.clone(),
        })
    }
}

/// Commit a validated draft. Callers run [`validate`] first and refuse to
/// commit while it reports a problem — this function does not re-check.
pub fn commit(draft: &DataSourceDraft) -> DataSourceSetting {
    DataSourceSetting {
        id: draft.id.clone(),
        name: draft.name.clone(),
        driver: draft.driver.clone(),
        group: draft.group.clone(),
        color: draft.color.clone(),
        host: draft.host.clone(),
        port: draft.port,
        database: draft.database.clone(),
        user: draft.user.clone(),
        auth: draft.auth.clone(),
        read_only: draft.read_only,
        history: draft.history,
        url: draft.url.clone(),
        ssl: SslSetting {
            mode: draft.ssl_mode.clone(),
            ca_file: draft.ssl_ca_file.clone(),
        },
        ssh: draft.ssh_setting(),
        options: Default::default(),
    }
}

/// Is `color` empty (no colour chosen) or a well-formed `#rrggbb`?
fn is_valid_color(color: &str) -> bool {
    if color.is_empty() {
        return true;
    }
    let hex = match color.strip_prefix('#') {
        Some(hex) => hex,
        None => return false,
    };
    hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit())
}

/// Every problem this draft has, checked against `other_ids` — every other
/// source's id in scope (global + project, this one's own id already
/// excluded by the caller) for the duplicate-id check.
pub fn validate(draft: &DataSourceDraft, other_ids: &[String]) -> Vec<DataSourceProblem> {
    let mut problems = Vec::new();

    if draft.name.trim().is_empty() {
        problems.push(DataSourceProblem {
            field: DataSourceField::Name,
            sentence: "Name must not be empty.".to_string(),
        });
    }

    if other_ids.iter().any(|id| id == &draft.id) {
        problems.push(DataSourceProblem {
            field: DataSourceField::Id,
            sentence: "A data source with this id already exists.".to_string(),
        });
    }

    if draft.port == Some(0) {
        problems.push(DataSourceProblem {
            field: DataSourceField::Port,
            sentence: "Port must be between 1 and 65535.".to_string(),
        });
    }

    if !is_valid_color(&draft.color) {
        problems.push(DataSourceProblem {
            field: DataSourceField::Color,
            sentence: "Colour must be empty or a #rrggbb hex value.".to_string(),
        });
    }

    if !draft.ssh_host.is_empty() && draft.ssh_user.is_empty() {
        problems.push(DataSourceProblem {
            field: DataSourceField::SshUser,
            sentence: "An SSH tunnel needs a user name.".to_string(),
        });
    }

    problems
}

/// A console file's default data source (`[database] file_sources`) must
/// name a path relative to the project root — an absolute path defeats the
/// point (a teammate's checkout has a different root) and is refused
/// rather than silently reinterpreted.
pub fn validate_file_source_path(path: &str) -> Option<DataSourceProblem> {
    if std::path::Path::new(path).is_absolute() {
        Some(DataSourceProblem {
            field: DataSourceField::FileSourcePath,
            sentence: "Console file path must be relative to the project root.".to_string(),
        })
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_draft() -> DataSourceDraft {
        let mut draft = DataSourceDraft::new("abc123".to_string(), "postgresql".to_string());
        draft.name = "prod".to_string();
        draft
    }

    #[test]
    fn a_valid_draft_has_no_problems() {
        assert!(validate(&valid_draft(), &[]).is_empty());
    }

    #[test]
    fn an_empty_name_is_a_problem() {
        let mut draft = valid_draft();
        draft.name = "   ".to_string();
        let problems = validate(&draft, &[]);
        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].field, DataSourceField::Name);
    }

    #[test]
    fn a_duplicate_id_is_a_problem() {
        let draft = valid_draft();
        let problems = validate(&draft, &["abc123".to_string(), "other".to_string()]);
        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].field, DataSourceField::Id);
    }

    #[test]
    fn a_zero_port_is_a_problem_but_none_and_a_real_port_are_not() {
        let mut draft = valid_draft();
        draft.port = Some(0);
        assert_eq!(validate(&draft, &[])[0].field, DataSourceField::Port);

        draft.port = None;
        assert!(validate(&draft, &[]).is_empty());

        draft.port = Some(5432);
        assert!(validate(&draft, &[]).is_empty());
    }

    #[test]
    fn an_invalid_color_is_a_problem() {
        let mut draft = valid_draft();
        for bad in ["red", "#fff", "#gggggg", "3a7bd5"] {
            draft.color = bad.to_string();
            let problems = validate(&draft, &[]);
            assert_eq!(problems.len(), 1, "expected a problem for {bad:?}");
            assert_eq!(problems[0].field, DataSourceField::Color);
        }
    }

    #[test]
    fn an_empty_or_well_formed_color_has_no_problem() {
        let mut draft = valid_draft();
        for good in ["", "#3a7bd5", "#000000", "#FFFFFF"] {
            draft.color = good.to_string();
            assert!(
                validate(&draft, &[]).is_empty(),
                "did not expect a problem for {good:?}"
            );
        }
    }

    #[test]
    fn an_ssh_host_without_a_user_is_a_problem() {
        let mut draft = valid_draft();
        draft.ssh_host = "bastion".to_string();
        let problems = validate(&draft, &[]);
        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].field, DataSourceField::SshUser);
    }

    #[test]
    fn an_ssh_host_with_a_user_has_no_problem() {
        let mut draft = valid_draft();
        draft.ssh_host = "bastion".to_string();
        draft.ssh_user = "florian".to_string();
        assert!(validate(&draft, &[]).is_empty());
    }

    #[test]
    fn a_blank_ssh_host_never_flags_a_missing_user() {
        assert!(validate(&valid_draft(), &[]).is_empty());
    }

    #[test]
    fn commit_round_trips_every_field_including_ssl_and_ssh() {
        let mut draft = valid_draft();
        draft.group = "work/analytics".to_string();
        draft.color = "#3a7bd5".to_string();
        draft.host = "db.internal".to_string();
        draft.port = Some(5432);
        draft.database = "shop".to_string();
        draft.user = "florian".to_string();
        draft.auth = "password".to_string();
        draft.read_only = true;
        draft.history = false;
        draft.ssl_mode = "verify-full".to_string();
        draft.ssl_ca_file = "/etc/ca.pem".to_string();
        draft.ssh_host = "bastion".to_string();
        draft.ssh_port = Some(22);
        draft.ssh_user = "florian".to_string();
        draft.ssh_auth = "agent".to_string();

        let setting = commit(&draft);
        assert_eq!(setting.id, "abc123");
        assert_eq!(setting.name, "prod");
        assert_eq!(setting.group, "work/analytics");
        assert_eq!(setting.color, "#3a7bd5");
        assert_eq!(setting.host, "db.internal");
        assert_eq!(setting.port, Some(5432));
        assert!(setting.read_only);
        assert!(!setting.history);
        assert_eq!(setting.ssl.mode, "verify-full");
        let ssh = setting.ssh.clone().expect("ssh configured");
        assert_eq!(ssh.host, "bastion");
        assert_eq!(ssh.user, "florian");

        let reopened = DataSourceDraft::from_setting(&setting);
        assert_eq!(reopened, draft);
    }

    #[test]
    fn a_relative_file_source_path_has_no_problem() {
        assert!(validate_file_source_path("sql/reports.sql").is_none());
    }

    #[test]
    fn an_absolute_file_source_path_is_a_problem() {
        let problem = validate_file_source_path("/etc/passwd").expect("a problem");
        assert_eq!(problem.field, DataSourceField::FileSourcePath);
    }
}
