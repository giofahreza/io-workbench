use iowb_protocol::{ProjectSessionsPage, ProjectSummary, SessionSummary};

use crate::api::PinnedSession;

pub enum Load<T> {
    Loading,
    Ready(T),
    Failed(String),
}

/// A project in the switcher. Its sessions are fetched page by page, only once
/// the project is expanded.
pub struct ProjectNode {
    pub project: ProjectSummary,
    pub expanded: bool,
    pub sessions: Vec<SessionSummary>,
    pub next_offset: usize,
    pub total: usize,
    pub has_more: bool,
    pub loaded: bool,
    pub loading: bool,
    pub error: Option<String>,
}

impl ProjectNode {
    pub fn new(project: ProjectSummary) -> Self {
        Self {
            project,
            expanded: false,
            sessions: Vec::new(),
            next_offset: 0,
            total: 0,
            has_more: false,
            loaded: false,
            loading: false,
            error: None,
        }
    }

    /// Adds one fetched page. Sessions that moved between pages (new activity
    /// reorders the list) are skipped rather than shown twice.
    pub fn absorb(&mut self, offset: usize, page: ProjectSessionsPage) {
        self.loading = false;
        self.error = None;
        self.loaded = true;
        self.total = page.total_count;
        self.has_more = page.has_more && !page.sessions.is_empty();
        self.next_offset = offset + page.sessions.len();
        for session in page.sessions {
            let known = self
                .sessions
                .iter()
                .any(|other| other.id == session.id && other.provider == session.provider);
            if !known {
                self.sessions.push(session);
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    Header(&'static str),
    Note(String),
    Pinned(usize),
    Project(usize),
    Session(usize, usize),
    More(usize),
}

impl Row {
    pub fn selectable(&self) -> bool {
        !matches!(self, Row::Header(_) | Row::Note(_))
    }
}

pub struct Switcher {
    pub pinned: Load<Vec<PinnedSession>>,
    pub projects: Load<Vec<ProjectNode>>,
    pub cursor: usize,
    pub filter: String,
}

impl Switcher {
    pub fn new() -> Self {
        Self {
            pinned: Load::Loading,
            projects: Load::Loading,
            cursor: 0,
            filter: String::new(),
        }
    }

    pub fn nodes(&self) -> Option<&Vec<ProjectNode>> {
        match &self.projects {
            Load::Ready(nodes) => Some(nodes),
            _ => None,
        }
    }

    pub fn nodes_mut(&mut self) -> Option<&mut Vec<ProjectNode>> {
        match &mut self.projects {
            Load::Ready(nodes) => Some(nodes),
            _ => None,
        }
    }

    pub fn rows(&self) -> Vec<Row> {
        let needle = self.filter.trim().to_lowercase();
        let hit = |texts: &[&str]| {
            needle.is_empty()
                || texts
                    .iter()
                    .any(|text| text.to_lowercase().contains(&needle))
        };

        let mut rows = vec![Row::Header("Pinned")];
        match &self.pinned {
            Load::Loading => rows.push(Row::Note("loading…".into())),
            Load::Failed(error) => rows.push(Row::Note(format!("could not load: {error}"))),
            Load::Ready(pinned) => {
                let before = rows.len();
                for (index, entry) in pinned.iter().enumerate() {
                    if hit(&[
                        &entry.title,
                        &entry.project,
                        &entry.provider,
                        &entry.session_id,
                    ]) {
                        rows.push(Row::Pinned(index));
                    }
                }
                if rows.len() == before {
                    let note = if pinned.is_empty() {
                        "no pinned chats"
                    } else {
                        "no matches"
                    };
                    rows.push(Row::Note(note.into()));
                }
            }
        }

        rows.push(Row::Header("Projects"));
        match &self.projects {
            Load::Loading => rows.push(Row::Note("loading…".into())),
            Load::Failed(error) => rows.push(Row::Note(format!("could not load: {error}"))),
            Load::Ready(nodes) => {
                let before = rows.len();
                for (index, node) in nodes.iter().enumerate() {
                    let name_hit = hit(&[&node.project.name]);
                    let session_hits: Vec<usize> = node
                        .sessions
                        .iter()
                        .enumerate()
                        .filter(|(_, session)| hit(&[&session.title, session.provider.as_str()]))
                        .map(|(position, _)| position)
                        .collect();
                    if !name_hit && session_hits.is_empty() {
                        continue;
                    }
                    rows.push(Row::Project(index));
                    let filtering = !needle.is_empty();
                    if !(node.expanded || (filtering && !session_hits.is_empty())) {
                        continue;
                    }
                    let shown: Vec<usize> = if filtering && !name_hit {
                        session_hits
                    } else {
                        (0..node.sessions.len()).collect()
                    };
                    rows.extend(
                        shown
                            .into_iter()
                            .map(|position| Row::Session(index, position)),
                    );
                    if let Some(error) = &node.error {
                        rows.push(Row::Note(format!("could not load: {error}")));
                    } else if node.loading {
                        rows.push(Row::Note("loading…".into()));
                    } else if node.has_more {
                        rows.push(Row::More(index));
                    } else if node.loaded && node.sessions.is_empty() {
                        rows.push(Row::Note("no sessions".into()));
                    }
                }
                if rows.len() == before {
                    let note = if nodes.is_empty() {
                        "no projects"
                    } else {
                        "no matches"
                    };
                    rows.push(Row::Note(note.into()));
                }
            }
        }
        rows
    }

    pub fn selected(&self) -> Option<Row> {
        self.rows()
            .into_iter()
            .nth(self.cursor)
            .filter(Row::selectable)
    }

    /// Keeps the cursor on a selectable row after the list changed.
    pub fn clamp(&mut self) {
        let rows = self.rows();
        if rows.get(self.cursor).is_some_and(Row::selectable) {
            return;
        }
        self.cursor = rows.iter().position(Row::selectable).unwrap_or(0);
    }

    pub fn step(&mut self, delta: isize) {
        let rows = self.rows();
        let mut index = self.cursor as isize;
        loop {
            index += delta;
            if index < 0 || index as usize >= rows.len() {
                return;
            }
            if rows[index as usize].selectable() {
                self.cursor = index as usize;
                return;
            }
        }
    }

    pub fn jump(&mut self, to_end: bool) {
        let rows = self.rows();
        let found = if to_end {
            rows.iter().rposition(Row::selectable)
        } else {
            rows.iter().position(Row::selectable)
        };
        if let Some(index) = found {
            self.cursor = index;
        }
    }

    /// Moves the cursor onto a project's own row.
    pub fn focus_project(&mut self, project_index: usize) {
        if let Some(position) = self
            .rows()
            .iter()
            .position(|row| *row == Row::Project(project_index))
        {
            self.cursor = position;
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use iowb_protocol::Provider;

    use super::*;

    fn project(name: &str) -> ProjectSummary {
        ProjectSummary {
            id: format!("id-{name}"),
            name: name.into(),
            path: format!("/work/{name}"),
            repo_name: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            sessions: Vec::new(),
        }
    }

    fn session(id: &str, title: &str) -> SessionSummary {
        SessionSummary {
            id: id.into(),
            title: title.into(),
            provider: Provider::Codex,
            ..Default::default()
        }
    }

    fn page(ids: &[&str], has_more: bool, total: usize) -> ProjectSessionsPage {
        ProjectSessionsPage {
            sessions: ids
                .iter()
                .map(|id| session(id, &format!("title {id}")))
                .collect(),
            has_more,
            total_count: total,
        }
    }

    fn switcher() -> Switcher {
        let mut switcher = Switcher::new();
        switcher.pinned = Load::Ready(vec![PinnedSession {
            session_id: "p1".into(),
            title: "Fix login".into(),
            project: "web".into(),
            provider: "codex".into(),
        }]);
        switcher.projects = Load::Ready(vec![
            ProjectNode::new(project("web")),
            ProjectNode::new(project("api")),
        ]);
        switcher
    }

    #[test]
    fn lists_pinned_chats_then_collapsed_projects() {
        let rows = switcher().rows();
        assert_eq!(
            rows,
            [
                Row::Header("Pinned"),
                Row::Pinned(0),
                Row::Header("Projects"),
                Row::Project(0),
                Row::Project(1),
            ]
        );
    }

    #[test]
    fn expanding_shows_sessions_and_a_load_more_row_until_the_last_page() {
        let mut switcher = switcher();
        let node = &mut switcher.nodes_mut().unwrap()[0];
        node.expanded = true;
        node.absorb(0, page(&["a", "b"], true, 3));
        assert_eq!(node.next_offset, 2);
        let rows = switcher.rows();
        assert!(rows.contains(&Row::Session(0, 1)) && rows.contains(&Row::More(0)));

        let node = &mut switcher.nodes_mut().unwrap()[0];
        node.absorb(2, page(&["b", "c"], false, 3));
        assert_eq!(node.sessions.len(), 3, "the repeated session is skipped");
        assert!(!switcher.rows().contains(&Row::More(0)));
    }

    #[test]
    fn filter_matches_pins_projects_and_loaded_sessions() {
        let mut switcher = switcher();
        let node = &mut switcher.nodes_mut().unwrap()[1];
        node.absorb(0, page(&["x"], false, 1));
        switcher.filter = "title x".into();
        let rows = switcher.rows();
        assert!(rows.contains(&Row::Project(1)) && rows.contains(&Row::Session(1, 0)));
        assert!(!rows.contains(&Row::Project(0)) && !rows.contains(&Row::Pinned(0)));

        switcher.filter = "nothing".into();
        assert!(
            switcher
                .rows()
                .iter()
                .filter(|row| **row == Row::Note("no matches".into()))
                .count()
                == 2
        );
    }

    #[test]
    fn cursor_skips_headers_and_notes() {
        let mut switcher = switcher();
        switcher.clamp();
        assert_eq!(switcher.selected(), Some(Row::Pinned(0)));
        switcher.step(1);
        assert_eq!(switcher.selected(), Some(Row::Project(0)));
        switcher.step(-1);
        assert_eq!(switcher.selected(), Some(Row::Pinned(0)));
        switcher.step(-1);
        assert_eq!(
            switcher.selected(),
            Some(Row::Pinned(0)),
            "stays on the first selectable row"
        );
        switcher.jump(true);
        assert_eq!(switcher.selected(), Some(Row::Project(1)));
    }
}
