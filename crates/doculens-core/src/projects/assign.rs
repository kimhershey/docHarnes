//! One assignment of many documents to many projects (1.0.0 plan §12 / §12-B). The "프로젝트에 넣기"
//! dialog sends every (document, project) pair it wants put in or taken out in one request: a document
//! may join several projects, each row of the sheet has its own kind (업무 문서 / 참고 자료) and a pair
//! that is unticked leaves the project.
//!
//! The membership rows and the roles change in one transaction. The tracked copies of documents that
//! sit in several projects ([`super::copies`]) are file work and follow after it, as they do for the
//! single-project commands: a copy that cannot be written does not undo the membership. The outcome
//! carries the request that puts everything back, so the caller can offer one "되돌리기".

use std::collections::BTreeMap;

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::db::Db;
use crate::documents::{repo as docs, DocumentRole};
use crate::error::Result;
use crate::knowledge::repo as knowledge_repo;
use crate::paths::WorkspacePaths;

use super::classify::Assignment;
use super::{copies, repo, Project};

/// A document's place in a project: among its work documents or in its reference group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Placement {
    pub doc_id: String,
    pub project_id: String,
    /// `true` = 참고 자료 (the project's reference group), `false` = 업무 문서.
    #[serde(default)]
    pub reference: bool,
}

/// A role to set after the memberships — the undo of an assignment restores the roles it changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleChange {
    pub doc_id: String,
    pub role: DocumentRole,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct AssignRequest {
    /// Pairs to take out, from the project's work documents and from its reference group. Applied first.
    pub remove: Vec<Assignment>,
    /// Places to put documents in. A document that is in the project on the other side moves over.
    pub add: Vec<Placement>,
    /// Roles to set last (written by [`AssignOutcome::undo`]; a dialog leaves it empty).
    pub roles: Vec<RoleChange>,
}

impl AssignRequest {
    pub fn is_empty(&self) -> bool {
        self.remove.is_empty() && self.add.is_empty() && self.roles.is_empty()
    }
}

/// What changed in one project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ProjectTally {
    pub project_id: String,
    pub project_name: String,
    /// Documents that were not in the project and are now.
    pub added: u32,
    /// Documents that left the project.
    pub removed: u32,
    /// Documents that stayed in the project and changed sides (업무 문서 ↔ 참고 자료).
    pub moved: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct AssignOutcome {
    /// The projects something changed in, in the order the request named them.
    pub projects: Vec<ProjectTally>,
    pub added: u32,
    pub removed: u32,
    pub moved: u32,
    /// Documents whose role (work / knowledge …) is different now.
    pub roles_changed: u32,
    /// Pairs that changed nothing: already in place, not there to take out, or an unknown document.
    pub unchanged: u32,
    /// Project ids of the request that no longer exist (their pairs were skipped).
    pub missing_projects: Vec<String>,
    /// Documents whose membership or role changed.
    pub doc_ids: Vec<String>,
    /// Reference groups whose members changed.
    pub group_ids: Vec<String>,
    /// The request that puts everything back the way it was.
    pub undo: AssignRequest,
}

fn is_work_member(conn: &Connection, project_id: &str, doc_id: &str) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT COUNT(*) FROM project_documents WHERE project_id = ?1 AND document_id = ?2",
        params![project_id, doc_id],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n > 0)
}

/// The project's reference group that holds the document, when it does.
fn reference_group_with(conn: &Connection, project_id: &str, doc_id: &str) -> rusqlite::Result<Option<String>> {
    let mut st = conn.prepare_cached(
        "SELECT c.id FROM collection_docs cd JOIN collections c ON c.id = cd.collection_id \
         WHERE c.project_id = ?1 AND cd.document_id = ?2 LIMIT 1",
    )?;
    let mut rows = st.query(params![project_id, doc_id])?;
    match rows.next()? {
        Some(r) => Ok(Some(r.get(0)?)),
        None => Ok(None),
    }
}

/// The bookkeeping of one request.
#[derive(Default)]
struct Run {
    out: AssignOutcome,
    projects: BTreeMap<String, Option<Project>>,
    /// The role each touched document had before the request changed it.
    roles_before: BTreeMap<String, DocumentRole>,
    /// `(document, project)` pairs whose copy ends / is due once the rows are written.
    drop_copies: Vec<(String, String)>,
    make_copies: Vec<(String, String)>,
}

impl Run {
    fn project(&mut self, conn: &Connection, id: &str) -> rusqlite::Result<Option<Project>> {
        if !self.projects.contains_key(id) {
            let p = repo::get_project(conn, id)?;
            if p.is_none() && !self.out.missing_projects.iter().any(|m| m == id) {
                self.out.missing_projects.push(id.to_string());
            }
            self.projects.insert(id.to_string(), p);
        }
        Ok(self.projects.get(id).cloned().flatten())
    }

    fn tally(&mut self, project: &Project) -> &mut ProjectTally {
        let at = match self.out.projects.iter().position(|t| t.project_id == project.id) {
            Some(i) => i,
            None => {
                self.out.projects.push(ProjectTally {
                    project_id: project.id.clone(),
                    project_name: project.name.clone(),
                    ..Default::default()
                });
                self.out.projects.len() - 1
            }
        };
        &mut self.out.projects[at]
    }

    fn touched(&mut self, doc_id: &str) {
        if !self.out.doc_ids.iter().any(|d| d == doc_id) {
            self.out.doc_ids.push(doc_id.to_string());
        }
    }

    fn group(&mut self, group_id: &str) {
        if !self.out.group_ids.iter().any(|g| g == group_id) {
            self.out.group_ids.push(group_id.to_string());
        }
    }

    fn set_role(
        &mut self,
        conn: &Connection,
        doc_id: &str,
        before: DocumentRole,
        role: DocumentRole,
    ) -> rusqlite::Result<()> {
        if before == role {
            return Ok(());
        }
        self.roles_before.entry(doc_id.to_string()).or_insert(before);
        docs::set_role(conn, doc_id, role)
    }

    fn remove(&mut self, conn: &Connection, pair: &Assignment) -> rusqlite::Result<()> {
        let Some(project) = self.project(conn, &pair.project_id)? else { return Ok(()) };
        let work = repo::remove_document(conn, &project.id, &pair.doc_id)?;
        let group = reference_group_with(conn, &project.id, &pair.doc_id)?;
        if let Some(group) = &group {
            knowledge_repo::remove_docs_from_collection(conn, group, std::slice::from_ref(&pair.doc_id))?;
            self.group(group);
        }
        if !work && group.is_none() {
            self.out.unchanged += 1;
            return Ok(());
        }
        if work {
            self.drop_copies.push((pair.doc_id.clone(), project.id.clone()));
        }
        self.tally(&project).removed += 1;
        self.out.removed += 1;
        self.touched(&pair.doc_id);
        // A document that was on both sides goes back as a work document (the side the screens show).
        self.out.undo.add.push(Placement {
            doc_id: pair.doc_id.clone(),
            project_id: project.id.clone(),
            reference: !work,
        });
        Ok(())
    }

    fn add(&mut self, conn: &Connection, place: &Placement) -> rusqlite::Result<()> {
        let Some(project) = self.project(conn, &place.project_id)? else { return Ok(()) };
        let Some(doc) = docs::get_document(conn, &place.doc_id)?.filter(|d| d.deleted_at.is_none()) else {
            self.out.unchanged += 1;
            return Ok(());
        };
        let work = is_work_member(conn, &project.id, &doc.id)?;
        let group = reference_group_with(conn, &project.id, &doc.id)?;
        let pair = || Assignment { doc_id: doc.id.clone(), project_id: project.id.clone() };
        let moved;
        if place.reference {
            if group.is_some() && !work {
                self.out.unchanged += 1;
                return Ok(());
            }
            if group.is_none() {
                let g = knowledge_repo::project_reference_group(conn, &project.id, &project.name)?;
                knowledge_repo::add_docs_to_collection(conn, &g.id, std::slice::from_ref(&doc.id))?;
                self.group(&g.id);
            }
            moved = work;
            if work {
                repo::remove_document(conn, &project.id, &doc.id)?;
                self.drop_copies.push((doc.id.clone(), project.id.clone()));
            }
            self.set_role(conn, &doc.id, doc.role, DocumentRole::Knowledge)?;
        } else {
            if work && group.is_none() {
                self.out.unchanged += 1;
                return Ok(());
            }
            if !work {
                let elsewhere: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM project_documents WHERE document_id = ?1 AND project_id <> ?2",
                    params![doc.id, project.id],
                    |r| r.get(0),
                )?;
                repo::add_documents(conn, &project.id, std::slice::from_ref(&doc.id))?;
                // The first project keeps the original as its copy; every further one gets a file.
                if elsewhere > 0 {
                    self.make_copies.push((doc.id.clone(), project.id.clone()));
                }
            }
            moved = group.is_some();
            if let Some(group) = &group {
                knowledge_repo::remove_docs_from_collection(conn, group, std::slice::from_ref(&doc.id))?;
                self.group(group);
                // The role follows the memberships, as the project screen's 참고 자료 → 업무 문서 switch
                // does: still in another reference group → still reference material.
                if doc.role == DocumentRole::Knowledge && !knowledge_repo::in_reference_group(conn, &doc.id)? {
                    self.set_role(conn, &doc.id, doc.role, DocumentRole::Work)?;
                }
            }
        }
        if moved {
            self.tally(&project).moved += 1;
            self.out.moved += 1;
            self.out.undo.add.push(Placement {
                doc_id: doc.id.clone(),
                project_id: project.id.clone(),
                reference: !place.reference,
            });
        } else {
            self.tally(&project).added += 1;
            self.out.added += 1;
            self.out.undo.remove.push(pair());
        }
        self.touched(&doc.id);
        Ok(())
    }

    fn role(&mut self, conn: &Connection, change: &RoleChange) -> rusqlite::Result<()> {
        let Some(doc) = docs::get_document(conn, &change.doc_id)? else { return Ok(()) };
        if doc.role != change.role {
            self.set_role(conn, &doc.id, doc.role, change.role)?;
            self.touched(&doc.id);
        }
        Ok(())
    }

    /// The roles that are different at the end of the request, for the outcome and its undo.
    fn settle_roles(&mut self, conn: &Connection) -> rusqlite::Result<()> {
        for (doc_id, before) in std::mem::take(&mut self.roles_before) {
            let now = docs::get_document(conn, &doc_id)?.map(|d| d.role);
            if now.is_some_and(|r| r != before) {
                self.out.roles_changed += 1;
                self.out.undo.roles.push(RoleChange { doc_id, role: before });
            }
        }
        Ok(())
    }
}

/// Applies an assignment: takes the `remove` pairs out, puts the `add` places in, sets the `roles`.
/// Pairs that change nothing are counted, not errors — the sheet may be a moment older than the data.
pub fn assign(db: &Db, paths: &WorkspacePaths, req: &AssignRequest) -> Result<AssignOutcome> {
    let mut run = Run::default();
    db.with_tx(|tx| {
        for pair in &req.remove {
            run.remove(tx, pair)?;
        }
        for place in &req.add {
            run.add(tx, place)?;
        }
        for change in &req.roles {
            run.role(tx, change)?;
        }
        run.settle_roles(tx)
    })?;
    // Copies after the rows: ended ones first, so a pair that left and came back gets a fresh file.
    for (doc_id, project_id) in &run.drop_copies {
        if let Err(e) = copies::drop_membership_copy(db, paths, project_id, doc_id) {
            log::warn!("copy of {doc_id} in project {project_id} was not removed: {e}");
        }
    }
    for (doc_id, project_id) in &run.make_copies {
        if let Err(e) = copies::write_membership_copy(db, paths, project_id, doc_id) {
            log::warn!("copy of {doc_id} for project {project_id} failed: {e}");
        }
    }
    Ok(run.out)
}
